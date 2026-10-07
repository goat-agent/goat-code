import { openTerminal } from "@goat/code-terminal";
import { runTui } from "@goat/code-tui";

if (!process.stdin.isTTY || !process.stdout.isTTY) {
  process.stderr.write("goat-code needs an interactive terminal. Run it in a terminal window.\n");
  process.exit(1);
}

const terminal = await openTerminal({ input: process.stdin, output: process.stdout });
try {
  await runTui(terminal);
} finally {
  terminal.close();
}
