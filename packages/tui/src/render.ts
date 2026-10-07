import { paint } from "./theme.ts";
import type { Block } from "./transcript.ts";

export interface Line {
  readonly text: string;
  readonly copy?: string;
}

export interface Renderer {
  render(block: Block, width: number): readonly Line[];
}

interface Cached {
  readonly version: number;
  readonly width: number;
  readonly lines: readonly Line[];
}

const gutter = "  ";
const resultLines = 4;

export function createRenderer(): Renderer {
  const cache = new Map<number, Cached>();
  return {
    render: (block, width) => {
      const cached = cache.get(block.id);
      if (cached?.version === block.version && cached.width === width) {
        return cached.lines;
      }
      const lines = draw(block, Math.max(1, width - gutter.length));
      cache.set(block.id, { version: block.version, width, lines });
      return lines;
    },
  };
}

function draw(block: Block, columns: number): readonly Line[] {
  if (block.kind === "user") {
    return wrap(block.text, columns).map((line, index) => ({
      text: `${index === 0 ? paint("dim", "> ") : gutter}${line}`,
      copy: line,
    }));
  }
  if (block.kind === "text") {
    return markdown(block.text, columns).map((line) => content(line));
  }
  if (block.kind === "reasoning") {
    return [content(paint("dim", block.streaming ? "Thinking" : "Thought"))];
  }
  if (block.kind === "tool") {
    return toolLines(block, columns).map((line) => content(line));
  }
  return wrap(block.text, columns).map((line) => content(paint(block.tone, line)));
}

function toolLines(
  block: Extract<Block, { readonly kind: "tool" }>,
  columns: number,
): readonly string[] {
  const header = clip(`${paint("dim", "$")} ${commandOf(block)}`, columns);
  if (block.result === undefined) {
    return [header];
  }
  const output = block.result.text.replace(/\n+$/u, "").split("\n");
  const shown = output.slice(-resultLines);
  const hidden = output.length - shown.length;
  const role = block.result.isError ? "error" : "dim";
  return [
    header,
    ...(hidden > 0 ? [paint("dim", `  … ${hidden} more lines`)] : []),
    ...shown.map((line) => paint(role, clip(`  ${line}`, columns))),
  ];
}

function commandOf(block: Extract<Block, { readonly kind: "tool" }>): string {
  if (block.name === "bash") {
    const command = /"command"\s*:\s*"((?:[^"\\]|\\.)*)/u.exec(block.input)?.[1];
    if (command !== undefined) {
      const lines = unescape(command).split("\n");
      return lines.length > 1 ? `${lines[0] ?? ""} …` : (lines[0] ?? "");
    }
    return "";
  }
  return `${block.name} ${block.input.replaceAll("\n", " ")}`;
}

function unescape(value: string): string {
  try {
    const parsed: unknown = JSON.parse(`"${value.replace(/\\$/u, "")}"`);
    return typeof parsed === "string" ? parsed : value;
  } catch {
    return value;
  }
}

function markdown(text: string, columns: number): readonly string[] {
  if (text === "") {
    return [];
  }
  const lines = Bun.markdown.ansi(text, { columns, hyperlinks: false }).split("\n");
  while (lines.length > 0 && Bun.stripANSI(lines.at(-1) ?? "").trim() === "") {
    lines.pop();
  }
  return lines.map((line) => clip(line, columns));
}

function wrap(text: string, columns: number): readonly string[] {
  return text
    .split("\n")
    .flatMap((line) => Bun.wrapAnsi(line, columns, { hard: true, trim: false }).split("\n"));
}

function clip(text: string, columns: number): string {
  return Bun.stringWidth(text) > columns ? Bun.sliceAnsi(text, 0, columns, "…") : text;
}

function content(text: string): Line {
  return { text: `${gutter}${text}`, copy: Bun.stripANSI(text) };
}
