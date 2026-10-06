import { expect, test } from "bun:test";
import { z } from "zod";
import { defineTool } from "./define.ts";
import type { ToolContext } from "./tool.ts";

const context: ToolContext = { signal: new AbortController().signal };

const echo = defineTool({
  name: "echo",
  description: "Returns the text.",
  input: z.object({
    text: z.string().describe("The text to return."),
    times: z.number().int().default(1),
  }),
  run: ({ text, times }) =>
    Promise.resolve({ parts: [{ type: "text", text: text.repeat(times) }], isError: false }),
});

test("describes the tool with a JSON Schema of its input", () => {
  expect(echo.definition).toEqual({
    type: "function",
    name: "echo",
    description: "Returns the text.",
    inputSchema: {
      type: "object",
      properties: {
        text: { type: "string", description: "The text to return." },
        times: {
          type: "integer",
          default: 1,
          minimum: Number.MIN_SAFE_INTEGER,
          maximum: Number.MAX_SAFE_INTEGER,
        },
      },
      required: ["text"],
    },
  });
});

test("runs with the parsed input", async () => {
  expect(await echo.call({ text: "ab", times: 2 }, context)).toEqual({
    parts: [{ type: "text", text: "abab" }],
    isError: false,
  });
  expect(await echo.call({ text: "ab" }, context)).toEqual({
    parts: [{ type: "text", text: "ab" }],
    isError: false,
  });
});

test("returns an error result instead of running when the input does not match", async () => {
  let ran = false;
  const tool = defineTool({
    name: "noop",
    description: "Does nothing.",
    input: z.object({ path: z.string() }),
    run: () => {
      ran = true;
      return Promise.resolve({ parts: [], isError: false });
    },
  });
  const output = await tool.call({ path: 3 }, context);
  expect(ran).toBe(false);
  expect(output.isError).toBe(true);
  const [part] = output.parts;
  expect(part?.type === "text" ? part.text : "").toContain("expected string, received number");
});
