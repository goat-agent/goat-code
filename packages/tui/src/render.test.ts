import { expect, test } from "bun:test";
import { createRenderer } from "./render.ts";
import type { Block } from "./transcript.ts";

function plain(block: Block, width: number): readonly string[] {
  return createRenderer()
    .render(block, width)
    .map((line) => Bun.stripANSI(line.text));
}

test("draws the user message after a dim marker and keeps continuation lines aligned", () => {
  expect(plain({ id: 1, version: 0, kind: "user", text: "first\nsecond" }, 40)).toEqual([
    "> first",
    "  second",
  ]);
});

test("draws markdown that never exceeds the width", () => {
  const text =
    "# Title\n\nSome **bold** text.\n\n```ts\nconst a = 'a very long line that does not fit';\n```";
  const lines = plain({ id: 1, version: 0, kind: "text", text, streaming: false }, 30);
  expect(lines[0]).toBe("  Title");
  for (const line of lines) {
    expect(Bun.stringWidth(line)).toBeLessThanOrEqual(30);
  }
});

test("draws a bash call as its command and the end of its output", () => {
  const block: Block = {
    id: 1,
    version: 0,
    kind: "tool",
    callId: "c1",
    name: "bash",
    input: '{"command":"bun test\\nbun run check"}',
    streaming: false,
    result: { text: "1\n2\n3\n4\n5\n6\nexit 0", isError: false },
  };
  expect(plain(block, 40)).toEqual([
    "  $ bun test …",
    "    … 3 more lines",
    "    4",
    "    5",
    "    6",
    "    exit 0",
  ]);
});

test("shows the command while its input is still streaming", () => {
  const block: Block = {
    id: 1,
    version: 0,
    kind: "tool",
    callId: "c1",
    name: "bash",
    input: '{"command":"git sta',
    streaming: true,
  };
  expect(plain(block, 40)).toEqual(["  $ git sta"]);
});

test("reuses lines until the block or the width changes", () => {
  const renderer = createRenderer();
  const block: Block = { id: 1, version: 0, kind: "notice", text: "Stopped.", tone: "dim" };
  const first = renderer.render(block, 40);
  expect(renderer.render(block, 40)).toBe(first);
  expect(renderer.render({ ...block, version: 1 }, 40)).not.toBe(first);
  expect(renderer.render(block, 50)).not.toBe(first);
});
