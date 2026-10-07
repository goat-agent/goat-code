import type { Key, Modifiers, Mouse, TerminalEvent } from "./event.ts";

export type Reply =
  | { readonly kind: "kitty-keyboard"; readonly flags: number }
  | { readonly kind: "device-attributes" };

export type Parsed =
  | Exclude<TerminalEvent, { readonly type: "resize" }>
  | { readonly type: "reply"; readonly reply: Reply };

const none: Modifiers = { shift: false, alt: false, ctrl: false, meta: false };

export function decodeCsi(body: string, terminator: string): readonly Parsed[] {
  if (body.startsWith("<") && (terminator === "M" || terminator === "m")) {
    const mouse = decodeMouse(body.slice(1), terminator === "M");
    return mouse === undefined ? [] : [{ type: "mouse", mouse }];
  }
  if (body.startsWith("?")) {
    if (terminator === "u") {
      return [{ type: "reply", reply: { kind: "kitty-keyboard", flags: Number(body.slice(1)) } }];
    }
    if (terminator === "c") {
      return [{ type: "reply", reply: { kind: "device-attributes" } }];
    }
    return [];
  }
  const decoded =
    terminator === "u"
      ? kittyKey(body)
      : terminator === "~"
        ? tildeKey(body)
        : terminator === "Z"
          ? key("tab", { ...none, shift: true })
          : letterKey(terminator, body);
  return decoded === undefined ? [] : [keyEvent(decoded)];
}

const letterKeys: Readonly<Record<string, string>> = {
  A: "up",
  B: "down",
  C: "right",
  D: "left",
  H: "home",
  F: "end",
  P: "f1",
  Q: "f2",
  R: "f3",
  S: "f4",
};

export function letterKey(terminator: string, body: string): Key | undefined {
  const name = letterKeys[terminator];
  if (name === undefined) {
    return undefined;
  }
  return key(name, modifiers(Number(body.split(";")[1] ?? "1")));
}

const tildeKeys: Readonly<Record<string, string>> = {
  "1": "home",
  "2": "insert",
  "3": "delete",
  "4": "end",
  "5": "pageup",
  "6": "pagedown",
  "7": "home",
  "8": "end",
  "11": "f1",
  "12": "f2",
  "13": "f3",
  "14": "f4",
  "15": "f5",
  "17": "f6",
  "18": "f7",
  "19": "f8",
  "20": "f9",
  "21": "f10",
  "23": "f11",
  "24": "f12",
};

function tildeKey(body: string): Key | undefined {
  const [number = "", modifier = "1"] = body.split(";");
  const name = tildeKeys[number];
  return name === undefined ? undefined : key(name, modifiers(Number(modifier)));
}

const kittyNames: Readonly<Record<number, string>> = {
  9: "tab",
  13: "enter",
  27: "escape",
  32: "space",
  127: "backspace",
};

function kittyKey(body: string): Key | undefined {
  const [codes = "", modifier = "1", text] = body.split(";");
  const code = Number(codes.split(":")[0]);
  if (
    !Number.isInteger(code) ||
    code < 0 ||
    code > 0x10_ffff ||
    (code >= 0xe0_00 && code <= 0xf8_ff)
  ) {
    return undefined;
  }
  const mods = modifiers(Number(modifier.split(":")[0]));
  const named = kittyNames[code];
  const inserts = !mods.ctrl && !mods.alt && !mods.meta;
  const typed =
    text === undefined
      ? named === undefined || named === "space"
        ? String.fromCodePoint(code)
        : undefined
      : String.fromCodePoint(...text.split(":").map(Number));
  return key(named ?? String.fromCodePoint(code), mods, inserts ? typed : undefined);
}

const wheelDirections = ["up", "down", "left", "right"] as const;
const buttons = ["left", "middle", "right"] as const;

function decodeMouse(body: string, pressed: boolean): Mouse | undefined {
  const [code, x, y] = body.split(";").map(Number);
  if (code === undefined || x === undefined || y === undefined) {
    return undefined;
  }
  const base = {
    shift: (code & 4) !== 0,
    alt: (code & 8) !== 0,
    ctrl: (code & 16) !== 0,
    meta: false,
    row: y - 1,
    column: x - 1,
  };
  if ((code & 64) !== 0) {
    return { ...base, action: "wheel", direction: wheelDirections[code & 3] ?? "up" };
  }
  const button = buttons[code & 3];
  if ((code & 32) !== 0) {
    return button === undefined ? { ...base, action: "move" } : { ...base, action: "drag", button };
  }
  return button === undefined
    ? undefined
    : { ...base, action: pressed ? "press" : "release", button };
}

function modifiers(value: number): Modifiers {
  const bits = Number.isInteger(value) && value > 1 ? value - 1 : 0;
  return {
    shift: (bits & 1) !== 0,
    alt: (bits & 2) !== 0,
    ctrl: (bits & 4) !== 0,
    meta: (bits & 8) !== 0 || (bits & 32) !== 0,
  };
}

export function control(code: number): Key {
  if (code === 0x0d) {
    return key("enter");
  }
  if (code === 0x09) {
    return key("tab");
  }
  if (code === 0x7f) {
    return key("backspace");
  }
  if (code === 0x00) {
    return key("space", { ...none, ctrl: true });
  }
  const name = String.fromCodePoint(code <= 0x1a ? code + 0x60 : code + 0x40);
  return key(name, { ...none, ctrl: true });
}

export function printable(grapheme: string): Key {
  return key(grapheme === " " ? "space" : grapheme, none, grapheme);
}

export function altKey(grapheme: string): Key {
  return key(grapheme === " " ? "space" : grapheme, { ...none, alt: true });
}

export function key(name: string, mods: Modifiers = none, text?: string): Key {
  return text === undefined ? { name, ...mods } : { name, ...mods, text };
}

export function keyEvent(value: Key): Parsed {
  return { type: "key", key: value };
}
