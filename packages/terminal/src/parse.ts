import {
  altKey,
  control,
  decodeCsi,
  key,
  keyEvent,
  letterKey,
  type Parsed,
  printable,
} from "./decode.ts";

export interface Parser {
  feed(text: string): readonly Parsed[];
  flush(): readonly Parsed[];
  isWaiting(): boolean;
}

interface Step {
  readonly length: number;
  readonly events: readonly Parsed[];
  readonly pasteStart?: true;
}

const esc = "\u001B";
const pasteEnd = `${esc}[201~`;
const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });

export function createParser(): Parser {
  let buffer = "";
  let paste: string | undefined;

  const drain = (final: boolean): readonly Parsed[] => {
    const events: Parsed[] = [];
    while (buffer.length > 0) {
      if (paste !== undefined) {
        const end = buffer.indexOf(pasteEnd);
        if (end === -1) {
          const keep = partialSuffix(buffer, pasteEnd);
          paste += buffer.slice(0, buffer.length - keep);
          buffer = buffer.slice(buffer.length - keep);
          break;
        }
        events.push({ type: "paste", text: normalizeNewlines(paste + buffer.slice(0, end)) });
        paste = undefined;
        buffer = buffer.slice(end + pasteEnd.length);
        continue;
      }
      const step = next(buffer, final);
      if (step === undefined) {
        break;
      }
      buffer = buffer.slice(step.length);
      events.push(...step.events);
      if (step.pasteStart === true) {
        paste = "";
      }
    }
    return events;
  };

  return {
    feed: (text) => {
      buffer += text;
      return drain(false);
    },
    flush: () => drain(true),
    isWaiting: () => paste === undefined && buffer.startsWith(esc),
  };
}

function next(input: string, final: boolean): Step | undefined {
  if (input.startsWith(esc)) {
    return escape(input, final);
  }
  const code = codeAt(input, 0);
  if (isControl(code)) {
    return { length: 1, events: [keyEvent(control(code))] };
  }
  const run = printableRun(input);
  const events: Parsed[] = [];
  for (const { segment } of segmenter.segment(run)) {
    events.push(keyEvent(printable(segment)));
  }
  return { length: run.length, events };
}

function escape(input: string, final: boolean): Step | undefined {
  if (input.length === 1) {
    return final ? { length: 1, events: [keyEvent(key("escape"))] } : undefined;
  }
  const second = input.charAt(1);
  if (second === "[") {
    return csi(input, final);
  }
  if (second === "O") {
    return ss3(input, final);
  }
  if (second === "]") {
    return terminated(input, final, true);
  }
  if (second === "P" || second === "_" || second === "^") {
    return terminated(input, final, false);
  }
  if (second === esc) {
    return { length: 1, events: [keyEvent(key("escape"))] };
  }
  const code = codeAt(input, 1);
  if (isControl(code)) {
    return { length: 2, events: [keyEvent({ ...control(code), alt: true })] };
  }
  let grapheme = input.charAt(1);
  for (const { segment } of segmenter.segment(printableRun(input.slice(1)))) {
    grapheme = segment;
    break;
  }
  return { length: 1 + grapheme.length, events: [keyEvent(altKey(grapheme))] };
}

function csi(input: string, final: boolean): Step | undefined {
  let index = 2;
  while (index < input.length && !isFinalByte(codeAt(input, index))) {
    index += 1;
  }
  if (index >= input.length) {
    return incomplete(input, final);
  }
  const body = input.slice(2, index);
  const terminator = input.charAt(index);
  if (body === "" && terminator === "M") {
    const length = index + 4;
    return input.length < length ? incomplete(input, final) : { length, events: [] };
  }
  if (body === "200" && terminator === "~") {
    return { length: index + 1, events: [], pasteStart: true };
  }
  return { length: index + 1, events: decodeCsi(body, terminator) };
}

function ss3(input: string, final: boolean): Step | undefined {
  if (input.length < 3) {
    return incomplete(input, final);
  }
  const decoded = letterKey(input.charAt(2), "");
  return { length: 3, events: decoded === undefined ? [] : [keyEvent(decoded)] };
}

function terminated(input: string, final: boolean, bell: boolean): Step | undefined {
  const st = input.indexOf(`${esc}\\`, 2);
  const bel = bell ? input.indexOf("\u0007", 2) : -1;
  const ends = [bel === -1 ? undefined : bel + 1, st === -1 ? undefined : st + 2].filter(
    (end) => end !== undefined,
  );
  if (ends.length === 0) {
    return incomplete(input, final);
  }
  return { length: Math.min(...ends), events: [] };
}

function incomplete(input: string, final: boolean): Step | undefined {
  return final ? { length: input.length, events: [] } : undefined;
}

function codeAt(input: string, index: number): number {
  return input.codePointAt(index) ?? 0;
}

function isFinalByte(code: number): boolean {
  return code >= 0x40 && code <= 0x7e;
}

function isControl(code: number): boolean {
  return code < 0x20 || code === 0x7f;
}

function printableRun(input: string): string {
  let end = 0;
  while (end < input.length && !isControl(codeAt(input, end))) {
    end += 1;
  }
  return input.slice(0, end);
}

function partialSuffix(input: string, marker: string): number {
  for (let length = Math.min(marker.length - 1, input.length); length > 0; length -= 1) {
    if (input.endsWith(marker.slice(0, length))) {
      return length;
    }
  }
  return 0;
}

function normalizeNewlines(text: string): string {
  return text.replaceAll("\r\n", "\n").replaceAll("\r", "\n");
}
