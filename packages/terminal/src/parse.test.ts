import { expect, test } from "bun:test";
import type { Key, Modifiers } from "./event.ts";
import type { Parsed } from "./decode.ts";
import { createParser } from "./parse.ts";

const none: Modifiers = { shift: false, alt: false, ctrl: false, meta: false };

function key(name: string, mods: Partial<Modifiers> = {}, text?: string): Parsed {
  const value: Key = { name, ...none, ...mods, ...(text === undefined ? {} : { text }) };
  return { type: "key", key: value };
}

function parse(...chunks: readonly string[]): readonly Parsed[] {
  const parser = createParser();
  return [...chunks.flatMap((chunk) => parser.feed(chunk)), ...parser.flush()];
}

test("splits typed text into graphemes", () => {
  expect(parse("a한👩‍👩‍👧 ")).toEqual([
    key("a", {}, "a"),
    key("한", {}, "한"),
    key("👩‍👩‍👧", {}, "👩‍👩‍👧"),
    key("space", {}, " "),
  ]);
});

test("reads control keys, telling Enter (CR) from Ctrl+J (LF)", () => {
  expect(parse("\r\n\t\u007F\u0003\u0008")).toEqual([
    key("enter"),
    key("j", { ctrl: true }),
    key("tab"),
    key("backspace"),
    key("c", { ctrl: true }),
    key("h", { ctrl: true }),
  ]);
});

test("waits for more input after a lone ESC and reads it as Esc on flush", () => {
  const parser = createParser();
  expect(parser.feed("\u001B")).toEqual([]);
  expect(parser.isWaiting()).toBe(true);
  expect(parser.flush()).toEqual([key("escape")]);
  expect(parser.isWaiting()).toBe(false);
});

test("reads ESC followed by a key as Alt", () => {
  expect(parse("\u001Bb\u001B\r")).toEqual([key("b", { alt: true }), key("enter", { alt: true })]);
});

test("reads cursor and editing keys with modifiers", () => {
  expect(
    parse("\u001B[A\u001B[1;5C\u001BOD\u001B[H\u001B[4~\u001B[5~\u001B[3;2~\u001B[15~\u001B[Z"),
  ).toEqual([
    key("up"),
    key("right", { ctrl: true }),
    key("left"),
    key("home"),
    key("end"),
    key("pageup"),
    key("delete", { shift: true }),
    key("f5"),
    key("tab", { shift: true }),
  ]);
});

test("reads kitty keyboard protocol keys", () => {
  expect(parse("\u001B[13;2u\u001B[97;5u\u001B[27u\u001B[32u\u001B[97;1;65u")).toEqual([
    key("enter", { shift: true }),
    key("a", { ctrl: true }),
    key("escape"),
    key("space", {}, " "),
    key("a", {}, "A"),
  ]);
});

test("reads a bracketed paste as one event, even across chunks", () => {
  expect(parse("\u001B[200~line one\r\nline ", "two\u001B[20", "1~x")).toEqual([
    { type: "paste", text: "line one\nline two" },
    key("x", {}, "x"),
  ]);
});

test("keeps escape sequences inside a paste as text", () => {
  expect(parse("\u001B[200~\u001B[A\r\u001B[201~")).toEqual([
    { type: "paste", text: "\u001B[A\n" },
  ]);
});

test("reads SGR mouse events", () => {
  expect(
    parse("\u001B[<64;10;5M\u001B[<65;10;5M\u001B[<0;3;2M\u001B[<0;3;2m\u001B[<32;4;2M"),
  ).toEqual([
    {
      type: "mouse",
      mouse: { ...none, action: "wheel", direction: "up", row: 4, column: 9 },
    },
    {
      type: "mouse",
      mouse: { ...none, action: "wheel", direction: "down", row: 4, column: 9 },
    },
    { type: "mouse", mouse: { ...none, action: "press", button: "left", row: 1, column: 2 } },
    { type: "mouse", mouse: { ...none, action: "release", button: "left", row: 1, column: 2 } },
    { type: "mouse", mouse: { ...none, action: "drag", button: "left", row: 1, column: 3 } },
  ]);
});

test("reports terminal replies and drops replies it does not use", () => {
  expect(
    parse("\u001B[?1u\u001B]11;rgb:0000/0000/0000\u0007\u001BP>|kitty\u001B\\\u001B[?62;22c"),
  ).toEqual([
    { type: "reply", reply: { kind: "kitty-keyboard", flags: 1 } },
    { type: "reply", reply: { kind: "device-attributes" } },
  ]);
});

test("joins a sequence split across chunks", () => {
  const parser = createParser();
  expect(parser.feed("\u001B[1;")).toEqual([]);
  expect(parser.isWaiting()).toBe(true);
  expect(parser.feed("5A")).toEqual([key("up", { ctrl: true })]);
});

test("drops legacy X10 mouse reports", () => {
  expect(parse("\u001B[M !!a")).toEqual([key("a", {}, "a")]);
});
