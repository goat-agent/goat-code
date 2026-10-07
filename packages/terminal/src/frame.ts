export interface Cursor {
  readonly row: number;
  readonly column: number;
}

export interface Frame {
  readonly lines: readonly string[];
  readonly cursor?: Cursor;
}

const csi = "\u001B[";
const beginUpdate = `${csi}?2026h`;
const endUpdate = `${csi}?2026l`;
const hideCursor = `${csi}?25l`;
const showCursor = `${csi}?25h`;
const reset = `${csi}0m`;

export function frameUpdate(previous: Frame | undefined, next: Frame, rows: number): string {
  const changed: string[] = [];
  for (let row = 0; row < rows; row += 1) {
    const line = next.lines[row] ?? "";
    if (previous === undefined ? line !== "" : (previous.lines[row] ?? "") !== line) {
      changed.push(`${moveTo(row, 0)}${reset}${csi}2K${line}`);
    }
  }
  const cursorMoved = !sameCursor(previous?.cursor, next.cursor);
  if (changed.length === 0 && previous !== undefined && !cursorMoved) {
    return "";
  }
  const cursor =
    next.cursor === undefined ? "" : `${moveTo(next.cursor.row, next.cursor.column)}${showCursor}`;
  const clear = previous === undefined ? `${csi}2J` : "";
  return `${beginUpdate}${hideCursor}${clear}${changed.join("")}${reset}${cursor}${endUpdate}`;
}

function moveTo(row: number, column: number): string {
  return `${csi}${row + 1};${column + 1}H`;
}

function sameCursor(a: Cursor | undefined, b: Cursor | undefined): boolean {
  return a?.row === b?.row && a?.column === b?.column;
}
