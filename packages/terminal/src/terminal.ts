import type { Parsed, Reply } from "./decode.ts";
import type { Size, TerminalEvent } from "./event.ts";
import { type Frame, frameUpdate } from "./frame.ts";
import { createParser } from "./parse.ts";

export interface TerminalInput {
  setRawMode(mode: boolean): unknown;
  on(event: "data", listener: (chunk: Uint8Array) => void): unknown;
  off(event: "data", listener: (chunk: Uint8Array) => void): unknown;
  resume(): unknown;
  pause(): unknown;
}

export interface TerminalOutput {
  readonly columns: number;
  readonly rows: number;
  write(text: string): unknown;
  on(event: "resize", listener: () => void): unknown;
  off(event: "resize", listener: () => void): unknown;
}

export interface TerminalOptions {
  readonly input: TerminalInput;
  readonly output: TerminalOutput;
  readonly escapeDelay?: number;
  readonly replyTimeout?: number;
}

export interface Capabilities {
  readonly kittyKeyboard: boolean;
}

export interface Terminal {
  readonly capabilities: Capabilities;
  readonly events: AsyncIterable<TerminalEvent>;
  size(): Size;
  draw(frame: Frame): void;
  close(): void;
}

const csi = "\u001B[";
const enterModes = `${csi}?1049h${csi}?2004h${csi}?1000h${csi}?1006h`;
const leaveModes = `${csi}?1006l${csi}?1000l${csi}?2004l${csi}0m${csi}?25h${csi}?1049l`;
const signals = ["SIGTERM", "SIGHUP"] as const;

export async function openTerminal(options: TerminalOptions): Promise<Terminal> {
  const { input, output } = options;
  const reader = createReader(options.escapeDelay ?? 50);
  const guard = createGuard(output);
  input.setRawMode(true);
  input.on("data", reader.read);
  input.resume();
  output.write(enterModes);
  const kittyKeyboard = await detectKittyKeyboard(output, reader, options.replyTimeout ?? 1_000);
  if (kittyKeyboard) {
    output.write(`${csi}>1u`);
    guard.popKeyboardOnRestore();
  }
  const screen = createScreen(output, reader);
  let closed = false;
  return {
    capabilities: { kittyKeyboard },
    events: reader.events,
    size: screen.size,
    draw: (frame) => {
      if (!closed) {
        screen.draw(frame);
      }
    },
    close: () => {
      if (closed) {
        return;
      }
      closed = true;
      input.off("data", reader.read);
      reader.close();
      screen.close();
      guard.restore();
      input.setRawMode(false);
      input.pause();
    },
  };
}

interface Reader {
  readonly events: AsyncIterable<TerminalEvent>;
  readonly read: (chunk: Uint8Array) => void;
  readonly push: (event: TerminalEvent) => void;
  listen(handler?: (reply: Reply) => void): void;
  close(): void;
}

function createReader(escapeDelay: number): Reader {
  const parser = createParser();
  const decoder = new TextDecoder();
  const queue = createQueue();
  let onReply: ((reply: Reply) => void) | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deliver = (parsed: readonly Parsed[]): void => {
    for (const item of parsed) {
      if (item.type === "reply") {
        onReply?.(item.reply);
      } else {
        queue.push(item);
      }
    }
  };
  return {
    events: queue.iterable,
    read: (chunk) => {
      clearTimeout(timer);
      deliver(parser.feed(decoder.decode(chunk, { stream: true })));
      if (parser.isWaiting()) {
        timer = setTimeout(() => {
          deliver(parser.flush());
        }, escapeDelay);
      }
    },
    push: queue.push,
    listen: (handler) => {
      onReply = handler;
    },
    close: () => {
      clearTimeout(timer);
      queue.close();
    },
  };
}

async function detectKittyKeyboard(
  output: TerminalOutput,
  reader: Reader,
  timeout: number,
): Promise<boolean> {
  return new Promise((resolve) => {
    let supported = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = (): void => {
      clearTimeout(timer);
      reader.listen();
      resolve(supported);
    };
    timer = setTimeout(finish, timeout);
    reader.listen((reply) => {
      if (reply.kind === "kitty-keyboard") {
        supported = true;
      } else {
        finish();
      }
    });
    output.write(`${csi}?u${csi}c`);
  });
}

interface Guard {
  popKeyboardOnRestore(): void;
  restore(): void;
}

function createGuard(output: TerminalOutput): Guard {
  let popKeyboard = false;
  let restored = false;
  function restore(): void {
    if (restored) {
      return;
    }
    restored = true;
    process.off("exit", restore);
    for (const signal of signals) {
      process.off(signal, onSignal);
    }
    output.write(`${popKeyboard ? `${csi}<u` : ""}${leaveModes}`);
  }
  function onSignal(signal: NodeJS.Signals): void {
    restore();
    process.kill(process.pid, signal);
  }
  process.once("exit", restore);
  for (const signal of signals) {
    process.once(signal, onSignal);
  }
  return {
    popKeyboardOnRestore: () => {
      popKeyboard = true;
    },
    restore,
  };
}

interface Screen {
  readonly size: () => Size;
  draw(frame: Frame): void;
  close(): void;
}

function createScreen(output: TerminalOutput, reader: Reader): Screen {
  let previous: Frame | undefined;
  const size = (): Size => ({ columns: output.columns, rows: output.rows });
  const onResize = (): void => {
    previous = undefined;
    reader.push({ type: "resize", size: size() });
  };
  output.on("resize", onResize);
  return {
    size,
    draw: (frame) => {
      const update = frameUpdate(previous, frame, output.rows);
      if (update !== "") {
        output.write(update);
      }
      previous = frame;
    },
    close: () => {
      output.off("resize", onResize);
    },
  };
}

interface Queue {
  readonly iterable: AsyncIterable<TerminalEvent>;
  readonly push: (event: TerminalEvent) => void;
  close(): void;
}

function createQueue(): Queue {
  const items: TerminalEvent[] = [];
  const waiting: ((event?: TerminalEvent) => void)[] = [];
  let ended = false;
  return {
    iterable: {
      [Symbol.asyncIterator]: () => ({
        next: async (): Promise<IteratorResult<TerminalEvent, undefined>> => {
          const item = items.shift() ?? (ended ? undefined : await nextPushed());
          return item === undefined
            ? { value: undefined, done: true }
            : { value: item, done: false };
        },
      }),
    },
    push: (event) => {
      if (ended) {
        return;
      }
      const deliver = waiting.shift();
      if (deliver === undefined) {
        items.push(event);
      } else {
        deliver(event);
      }
    },
    close: () => {
      ended = true;
      for (const deliver of waiting.splice(0)) {
        deliver();
      }
    },
  };

  async function nextPushed(): Promise<TerminalEvent | undefined> {
    return new Promise((resolve) => {
      waiting.push(resolve);
    });
  }
}
