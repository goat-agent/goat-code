import { expect, test } from "bun:test";
import { type Frame, frameUpdate } from "./frame.ts";

interface Screen {
  readonly rows: readonly string[];
  readonly cursor: { readonly row: number; readonly column: number } | undefined;
  readonly cleared: number;
}

const esc = String.fromCodePoint(27);
const sequence = new RegExp(`${esc}\\[([?\\d;]*)([A-Za-z])|([^${esc}]+)`, "gu");

function apply(screen: Screen, output: string): Screen {
  const rows = [...screen.rows];
  let { cleared } = screen;
  let row = 0;
  let column = 0;
  let visible = screen.cursor !== undefined;
  for (const [, params = "", command, text] of output.matchAll(sequence)) {
    if (text !== undefined) {
      rows[row] = (rows[row] ?? "").slice(0, column) + text;
      column += text.length;
    } else if (command === "H") {
      const [r = "1", c = "1"] = params.split(";");
      row = Number(r) - 1;
      column = Number(c) - 1;
    } else if (command === "K" && params === "2") {
      rows[row] = "";
    } else if (command === "J" && params === "2") {
      rows.fill("");
      cleared += 1;
    } else if (params === "?25") {
      visible = command === "h";
    }
  }
  return { rows, cursor: visible ? { row, column } : undefined, cleared };
}

function screenOf(rows: number): Screen {
  return { rows: Array.from({ length: rows }, () => ""), cursor: undefined, cleared: 0 };
}

test("draws every line of the first frame inside one synchronized update", () => {
  const output = frameUpdate(undefined, { lines: ["one", "two"] }, 3);
  expect(output.startsWith("\u001B[?2026h")).toBe(true);
  expect(output.endsWith("\u001B[?2026l")).toBe(true);
  const screen = apply(screenOf(3), output);
  expect(screen).toEqual({ rows: ["one", "two", ""], cursor: undefined, cleared: 1 });
});

test("writes only the lines that changed", () => {
  const first: Frame = { lines: ["one", "two", "three"] };
  const second: Frame = { lines: ["one", "TWO", "three"] };
  const output = frameUpdate(first, second, 3);
  expect(output).toContain("TWO");
  expect(output).not.toContain("one");
  expect(output).not.toContain("three");
  const screen = apply(apply(screenOf(3), frameUpdate(undefined, first, 3)), output);
  expect(screen.rows).toEqual(["one", "TWO", "three"]);
});

test("clears rows that the new frame no longer fills", () => {
  const first: Frame = { lines: ["one", "two", "three"] };
  const drawn = apply(screenOf(3), frameUpdate(undefined, first, 3));
  expect(apply(drawn, frameUpdate(first, { lines: ["one"] }, 3)).rows).toEqual(["one", "", ""]);
});

test("places and shows the cursor, and hides it when the frame has none", () => {
  const typing: Frame = { lines: ["> 한글", ""], cursor: { row: 0, column: 6 } };
  const shown = apply(screenOf(2), frameUpdate(undefined, typing, 2));
  expect(shown.cursor).toEqual({ row: 0, column: 6 });
  expect(apply(shown, frameUpdate(typing, { lines: ["> 한글", ""] }, 2)).cursor).toBeUndefined();
});

test("writes nothing when neither the lines nor the cursor changed", () => {
  const frame: Frame = { lines: ["same"], cursor: { row: 0, column: 4 } };
  expect(frameUpdate(frame, { lines: ["same"], cursor: { row: 0, column: 4 } }, 1)).toBe("");
});

test("resets styles before clearing each line, so a background color does not spread", () => {
  const output = frameUpdate(undefined, { lines: ["\u001B[41mred"] }, 1);
  expect(output).toContain("\u001B[0m\u001B[2K\u001B[41mred");
});
