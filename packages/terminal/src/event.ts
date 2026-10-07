export interface Modifiers {
  readonly shift: boolean;
  readonly alt: boolean;
  readonly ctrl: boolean;
  readonly meta: boolean;
}

export interface Key extends Modifiers {
  readonly name: string;
  readonly text?: string;
}

interface Position {
  readonly row: number;
  readonly column: number;
}

export type Mouse = Modifiers &
  Position &
  (
    | {
        readonly action: "press" | "release" | "drag";
        readonly button: "left" | "middle" | "right";
      }
    | { readonly action: "move" }
    | { readonly action: "wheel"; readonly direction: "up" | "down" | "left" | "right" }
  );

export interface Size {
  readonly columns: number;
  readonly rows: number;
}

export type TerminalEvent =
  | { readonly type: "key"; readonly key: Key }
  | { readonly type: "paste"; readonly text: string }
  | { readonly type: "mouse"; readonly mouse: Mouse }
  | { readonly type: "resize"; readonly size: Size };
