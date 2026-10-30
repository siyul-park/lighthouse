export const VERSION = "1";

export function slug(text: string): string {
  return text.trim().toLowerCase();
}

export default function main(): string {
  return VERSION;
}

export type Id = string;
