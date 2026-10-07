export type Role = "text" | "dim" | "accent" | "error" | "warning" | "border";

const csi = "\u001B[";

const styles: Readonly<Record<Role, readonly [string, string]>> = {
  text: ["", ""],
  dim: [`${csi}2m`, `${csi}22m`],
  accent: [`${csi}36m`, `${csi}39m`],
  error: [`${csi}31m`, `${csi}39m`],
  warning: [`${csi}33m`, `${csi}39m`],
  border: [`${csi}90m`, `${csi}39m`],
};

export function paint(role: Role, text: string): string {
  const [open, close] = styles[role];
  return text === "" || open === "" ? text : `${open}${text}${close}`;
}
