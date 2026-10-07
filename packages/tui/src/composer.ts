import { paint } from "./theme.ts";

interface Token {
  readonly label: string;
  readonly text: string;
}

export interface Composer {
  readonly text: string;
  readonly cursor: number;
  readonly tokens: ReadonlyMap<string, Token>;
  readonly pastes: number;
}

export const emptyComposer: Composer = { text: "", cursor: 0, tokens: new Map(), pastes: 0 };

const pasteLineLimit = 10;
const tokenBase = 0x10_00_00;
const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });

export function insert(state: Composer, text: string): Composer {
  return {
    ...state,
    text: state.text.slice(0, state.cursor) + text + state.text.slice(state.cursor),
    cursor: state.cursor + text.length,
  };
}

export function paste(state: Composer, text: string): Composer {
  const lines = text.split("\n").length;
  if (lines <= pasteLineLimit) {
    return insert(state, text);
  }
  const number = state.pastes + 1;
  const marker = String.fromCodePoint(tokenBase + number);
  const token = { label: `[Pasted text #${number} +${lines} lines]`, text };
  return insert(
    { ...state, tokens: new Map(state.tokens).set(marker, token), pastes: number },
    marker,
  );
}

export function expand(state: Composer): string {
  let text = "";
  for (const { segment } of segmenter.segment(state.text)) {
    text += state.tokens.get(segment)?.text ?? segment;
  }
  return text;
}

export function backspace(state: Composer): Composer {
  return remove(state, previousBoundary(state.text, state.cursor), state.cursor);
}

export function deleteForward(state: Composer): Composer {
  return remove(state, state.cursor, nextBoundary(state.text, state.cursor));
}

export function deleteWordBack(state: Composer): Composer {
  return remove(state, wordStart(state.text, state.cursor), state.cursor);
}

export function deleteToLineStart(state: Composer): Composer {
  return remove(state, lineStart(state.text, state.cursor), state.cursor);
}

export function deleteToLineEnd(state: Composer): Composer {
  return remove(state, state.cursor, lineEnd(state.text, state.cursor));
}

export function moveLeft(state: Composer): Composer {
  return moveTo(state, previousBoundary(state.text, state.cursor));
}

export function moveRight(state: Composer): Composer {
  return moveTo(state, nextBoundary(state.text, state.cursor));
}

export function moveWordLeft(state: Composer): Composer {
  return moveTo(state, wordStart(state.text, state.cursor));
}

export function moveWordRight(state: Composer): Composer {
  return moveTo(state, wordEnd(state.text, state.cursor));
}

export function moveLineStart(state: Composer): Composer {
  return moveTo(state, lineStart(state.text, state.cursor));
}

export function moveLineEnd(state: Composer): Composer {
  return moveTo(state, lineEnd(state.text, state.cursor));
}

export function moveUp(state: Composer): Composer {
  const start = lineStart(state.text, state.cursor);
  if (start === 0) {
    return state;
  }
  const previous = lineStart(state.text, start - 1);
  return moveTo(
    state,
    atColumn(state.text, previous, start - 1, column(state.text, start, state.cursor)),
  );
}

export function moveDown(state: Composer): Composer {
  const end = lineEnd(state.text, state.cursor);
  if (end === state.text.length) {
    return state;
  }
  const wanted = column(state.text, lineStart(state.text, state.cursor), state.cursor);
  return moveTo(state, atColumn(state.text, end + 1, lineEnd(state.text, end + 1), wanted));
}

export interface ComposerView {
  readonly lines: readonly string[];
  readonly cursor: { readonly row: number; readonly column: number };
}

export function renderComposer(
  state: Composer,
  width: number,
  maxRows: number,
  placeholder = "",
): ComposerView {
  const inner = Math.max(1, width - 4);
  const { rows, cursorRow, cursorColumn } =
    state.text === ""
      ? {
          rows: [paint("dim", Bun.sliceAnsi(placeholder, 0, inner))],
          cursorRow: 0,
          cursorColumn: 0,
        }
      : layout(state, inner);
  const height = Math.min(Math.max(rows.length, 1), Math.max(maxRows, 1));
  const first = Math.max(0, cursorRow - height + 1);
  const visible = rows.slice(first, first + height);
  return {
    lines: [
      border(`╭${"─".repeat(width - 2)}╮`),
      ...visible.map(
        (row) =>
          `${border("│")} ${row}${" ".repeat(Math.max(0, inner - Bun.stringWidth(row)))} ${border("│")}`,
      ),
      border(`╰${"─".repeat(width - 2)}╯`),
    ],
    cursor: { row: 1 + cursorRow - first, column: 2 + cursorColumn },
  };
}

function border(text: string): string {
  return paint("border", text);
}

interface Layout {
  readonly rows: readonly string[];
  readonly cursorRow: number;
  readonly cursorColumn: number;
}

function layout(state: Composer, inner: number): Layout {
  const rows: string[] = [];
  let row = "";
  let used = 0;
  let index = 0;
  let cursor = { row: 0, column: 0 };
  const breakRow = (): void => {
    rows.push(row);
    row = "";
    used = 0;
  };
  for (const { segment } of segmenter.segment(state.text)) {
    if (index === state.cursor) {
      cursor = { row: rows.length, column: used };
    }
    index += segment.length;
    if (segment === "\n") {
      breakRow();
      continue;
    }
    const token = state.tokens.get(segment);
    const shown = token === undefined ? segment : Bun.sliceAnsi(token.label, 0, inner);
    const width = Bun.stringWidth(shown);
    if (used + width > inner && used > 0) {
      if (index - segment.length === state.cursor) {
        cursor = { row: rows.length + 1, column: 0 };
      }
      breakRow();
    }
    row += token === undefined ? shown : paint("dim", shown);
    used += width;
  }
  if (index === state.cursor) {
    cursor = { row: rows.length, column: used };
  }
  rows.push(row);
  return { rows, cursorRow: cursor.row, cursorColumn: cursor.column };
}

function remove(state: Composer, from: number, to: number): Composer {
  if (from >= to) {
    return state;
  }
  return { ...state, text: state.text.slice(0, from) + state.text.slice(to), cursor: from };
}

function moveTo(state: Composer, cursor: number): Composer {
  return cursor === state.cursor ? state : { ...state, cursor };
}

function previousBoundary(text: string, index: number): number {
  let boundary = 0;
  for (const { index: start } of segmenter.segment(text.slice(0, index))) {
    boundary = start;
  }
  return index === 0 ? 0 : boundary;
}

function nextBoundary(text: string, index: number): number {
  for (const { segment } of segmenter.segment(text.slice(index))) {
    return index + segment.length;
  }
  return index;
}

function lineStart(text: string, index: number): number {
  return text.lastIndexOf("\n", index - 1) + 1;
}

function lineEnd(text: string, index: number): number {
  const end = text.indexOf("\n", index);
  return end === -1 ? text.length : end;
}

function column(text: string, start: number, index: number): number {
  return [...segmenter.segment(text.slice(start, index))].length;
}

function atColumn(text: string, start: number, end: number, target: number): number {
  let index = start;
  let count = 0;
  for (const { segment } of segmenter.segment(text.slice(start, end))) {
    if (count === target) {
      break;
    }
    index += segment.length;
    count += 1;
  }
  return index;
}

function wordStart(text: string, index: number): number {
  let position = index;
  while (position > 0 && /\s/u.test(text.charAt(position - 1))) {
    position -= 1;
  }
  while (position > 0 && !/\s/u.test(text.charAt(position - 1))) {
    position -= 1;
  }
  return position;
}

function wordEnd(text: string, index: number): number {
  let position = index;
  while (position < text.length && /\s/u.test(text.charAt(position))) {
    position += 1;
  }
  while (position < text.length && !/\s/u.test(text.charAt(position))) {
    position += 1;
  }
  return position;
}
