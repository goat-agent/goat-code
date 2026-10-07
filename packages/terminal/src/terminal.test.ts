import { expect, test } from "bun:test";
import type { TerminalEvent } from "./event.ts";
import {
  openTerminal,
  type Terminal,
  type TerminalInput,
  type TerminalOutput,
} from "./terminal.ts";

interface FakeInput extends TerminalInput {
  readonly modes: boolean[];
  send(text: string): void;
}

interface FakeOutput extends TerminalOutput {
  readonly written: string[];
  columns: number;
  rows: number;
  resize(columns: number, rows: number): void;
}

function fakeInput(): FakeInput {
  const listeners = new Set<(chunk: Uint8Array) => void>();
  const modes: boolean[] = [];
  return {
    modes,
    setRawMode: (mode) => modes.push(mode),
    on: (_event, listener) => listeners.add(listener),
    off: (_event, listener) => listeners.delete(listener),
    resume: () => true,
    pause: () => true,
    send: (text) => {
      for (const listener of listeners) {
        listener(new TextEncoder().encode(text));
      }
    },
  };
}

function fakeOutput(reply: (written: string) => void): FakeOutput {
  const listeners = new Set<() => void>();
  const output: FakeOutput = {
    written: [],
    columns: 80,
    rows: 24,
    write: (text) => {
      output.written.push(text);
      reply(text);
    },
    on: (_event, listener) => listeners.add(listener),
    off: (_event, listener) => listeners.delete(listener),
    resize: (columns, rows) => {
      output.columns = columns;
      output.rows = rows;
      for (const listener of listeners) {
        listener();
      }
    },
  };
  return output;
}

async function open(
  answer: string | undefined,
  options: { readonly replyTimeout?: number } = {},
): Promise<{ terminal: Terminal; input: FakeInput; output: FakeOutput }> {
  const input = fakeInput();
  const output = fakeOutput((written) => {
    if (answer !== undefined && written.endsWith("\u001B[c")) {
      queueMicrotask(() => {
        input.send(answer);
      });
    }
  });
  const terminal = await openTerminal({ input, output, escapeDelay: 10, ...options });
  return { terminal, input, output };
}

async function nextEvent(terminal: Terminal): Promise<TerminalEvent | undefined> {
  const result = await terminal.events[Symbol.asyncIterator]().next();
  return result.done === true ? undefined : result.value;
}

test("enters raw mode and the alternate screen, then restores both on close", async () => {
  const { terminal, input, output } = await open("\u001B[?62c");
  expect(input.modes).toEqual([true]);
  expect(output.written[0]).toBe("\u001B[?1049h\u001B[?2004h\u001B[?1000h\u001B[?1006h");
  terminal.close();
  terminal.close();
  expect(input.modes).toEqual([true, false]);
  expect(output.written.at(-1)).toBe(
    "\u001B[?1006l\u001B[?1000l\u001B[?2004l\u001B[0m\u001B[?25h\u001B[?1049l",
  );
  expect(output.written.filter((text) => text.includes("?1049l"))).toHaveLength(1);
});

test("turns on the kitty keyboard protocol when the terminal answers before DA1", async () => {
  const { terminal, output } = await open("\u001B[?0u\u001B[?62c");
  expect(terminal.capabilities).toEqual({ kittyKeyboard: true });
  expect(output.written).toContain("\u001B[>1u");
  terminal.close();
  expect(output.written.at(-1)?.startsWith("\u001B[<u")).toBe(true);
});

test("treats a terminal that answers only DA1 as legacy", async () => {
  const { terminal, output } = await open("\u001B[?62c");
  expect(terminal.capabilities).toEqual({ kittyKeyboard: false });
  expect(output.written).not.toContain("\u001B[>1u");
  terminal.close();
});

test("stops waiting for replies after the timeout when DA1 never answers", async () => {
  const { terminal } = await open(undefined, { replyTimeout: 20 });
  expect(terminal.capabilities).toEqual({ kittyKeyboard: false });
  terminal.close();
});

test("delivers input as events and a lone ESC after the delay", async () => {
  const { terminal, input } = await open("\u001B[?62c");
  input.send("a");
  expect(await nextEvent(terminal)).toEqual({
    type: "key",
    key: { name: "a", text: "a", shift: false, alt: false, ctrl: false, meta: false },
  });
  input.send("\u001B");
  expect(await nextEvent(terminal)).toEqual({
    type: "key",
    key: { name: "escape", shift: false, alt: false, ctrl: false, meta: false },
  });
  terminal.close();
});

test("draws only changes, and redraws everything after a resize", async () => {
  const { terminal, output } = await open("\u001B[?62c");
  const before = output.written.length;
  terminal.draw({ lines: ["hello"] });
  terminal.draw({ lines: ["hello"] });
  expect(output.written.length).toBe(before + 1);
  output.resize(100, 30);
  expect(await nextEvent(terminal)).toEqual({
    type: "resize",
    size: { columns: 100, rows: 30 },
  });
  terminal.draw({ lines: ["hello"] });
  expect(output.written.at(-1)).toContain("\u001B[2J");
  terminal.close();
});

test("ends the event stream on close", async () => {
  const { terminal } = await open("\u001B[?62c");
  const pending = nextEvent(terminal);
  terminal.close();
  expect(await pending).toBeUndefined();
});
