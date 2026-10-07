import { expect, test } from "bun:test";
import { emptyComposer } from "./composer.ts";
import { layout, type View } from "./layout.ts";
import { createRenderer } from "./render.ts";
import type { Block } from "./transcript.ts";

function notices(count: number): readonly Block[] {
  return Array.from({ length: count }, (_, index) => ({
    id: index + 1,
    version: 0,
    kind: "notice",
    text: `message ${index + 1}`,
    tone: "dim",
  }));
}

function view(blocks: readonly Block[], top?: number): View {
  return { blocks, composer: emptyComposer, top, hint: "" };
}

function plain(lines: readonly string[]): readonly string[] {
  return lines.map((line) => Bun.stripANSI(line));
}

test("fills the screen with the transcript above the composer", () => {
  const { frame, placement } = layout(view(notices(2)), { columns: 20, rows: 8 }, createRenderer());
  expect(plain(frame.lines)).toEqual([
    "  message 1",
    "",
    "  message 2",
    "",
    "",
    "╭──────────────────╮",
    "│                  │",
    "╰──────────────────╯",
  ]);
  expect(frame.cursor).toEqual({ row: 6, column: 2 });
  expect(placement).toEqual({ top: 0, height: 5, total: 3 });
});

test("follows the end of a long transcript", () => {
  const { frame, placement } = layout(
    view(notices(10)),
    { columns: 20, rows: 6 },
    createRenderer(),
  );
  expect(plain(frame.lines).slice(0, 3)).toEqual(["  message 9", "", "  message 10"]);
  expect(placement).toEqual({ top: 16, height: 3, total: 19 });
});

test("keeps a scrolled position and clamps it to the transcript", () => {
  const renderer = createRenderer();
  const scrolled = layout(view(notices(10), 2), { columns: 20, rows: 6 }, renderer);
  expect(plain(scrolled.frame.lines).slice(0, 3)).toEqual(["  message 2", "", "  message 3"]);
  const beyond = layout(view(notices(10), 99), { columns: 20, rows: 6 }, renderer);
  expect(beyond.placement.top).toBe(16);
});
