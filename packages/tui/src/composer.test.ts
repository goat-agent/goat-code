import { expect, test } from "bun:test";
import {
  backspace,
  type Composer,
  deleteWordBack,
  emptyComposer,
  expand,
  insert,
  moveDown,
  moveLeft,
  moveUp,
  paste,
  renderComposer,
} from "./composer.ts";

function typed(text: string): Composer {
  return insert(emptyComposer, text);
}

function plain(lines: readonly string[]): readonly string[] {
  return lines.map((line) => Bun.stripANSI(line));
}

test("deletes and moves by whole graphemes", () => {
  const state = typed("a한👩‍👩‍👧");
  expect(backspace(state).text).toBe("a한");
  expect(backspace(moveLeft(state)).text).toBe("a👩‍👩‍👧");
});

test("deletes the word before the cursor", () => {
  expect(deleteWordBack(typed("git commit -m")).text).toBe("git commit ");
});

test("moves between lines and keeps the column", () => {
  const state = { ...typed("abcd\nxy\n1234"), cursor: 3 };
  const down = moveDown(state);
  expect(down.cursor).toBe(7);
  expect(moveDown(down).cursor).toBe(10);
  expect(moveUp(moveDown(down)).cursor).toBe(7);
});

test("folds a long paste into a token and expands it on submit", () => {
  const long = Array.from({ length: 12 }, (_, index) => `line ${index}`).join("\n");
  const state = insert(paste(typed("see "), long), " now");
  expect(expand(state)).toBe(`see ${long} now`);
  const view = renderComposer(state, 60, 5);
  expect(plain(view.lines)[1]).toContain("see [Pasted text #1 +12 lines] now");
  expect(backspace({ ...state, cursor: state.text.length - 4 }).text).toBe("see  now");
  expect(paste(emptyComposer, "two\nlines").text).toBe("two\nlines");
});

test("draws a rounded box that fills the width", () => {
  const view = renderComposer(typed("hello"), 20, 5);
  expect(plain(view.lines)).toEqual([
    "╭──────────────────╮",
    "│ hello            │",
    "╰──────────────────╯",
  ]);
  expect(view.cursor).toEqual({ row: 1, column: 7 });
});

test("places the cursor after wide characters and wraps long lines", () => {
  expect(renderComposer(typed("한글"), 20, 5).cursor).toEqual({ row: 1, column: 6 });
  const wrapped = renderComposer(typed("abcdefghij"), 10, 5);
  expect(plain(wrapped.lines).slice(1, -1)).toEqual(["│ abcdef │", "│ ghij   │"]);
  expect(wrapped.cursor).toEqual({ row: 2, column: 6 });
});

test("scrolls inside the box to keep the cursor visible", () => {
  const view = renderComposer(typed("1\n2\n3\n4\n5"), 20, 3);
  expect(plain(view.lines).slice(1, -1)).toEqual([
    "│ 3                │",
    "│ 4                │",
    "│ 5                │",
  ]);
  expect(view.cursor.row).toBe(3);
});

test("shows the hint as a dim placeholder when empty", () => {
  const view = renderComposer(emptyComposer, 40, 3, "Press Ctrl+C again to exit");
  expect(plain(view.lines)[1]).toBe("│ Press Ctrl+C again to exit           │");
  expect(view.cursor).toEqual({ row: 1, column: 2 });
});
