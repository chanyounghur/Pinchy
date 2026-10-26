// The desktop's palette (src/App.css) on a phone: iOS grouped background,
// white cards, coral accent. Colors are chosen to pass 4.5:1 for text.

import { useColorScheme } from "react-native";

export interface Theme {
  dark: boolean;
  bg: string;
  fg: string;
  muted: string;
  chip: string;
  chipStrong: string;
  card: string;
  cardFg: string;
  cardMuted: string;
  separator: string;
  accent: string;
  accentStrong: string;
  origin: string;
  danger: string;
  ok: string;
}

const light: Theme = {
  dark: false,
  bg: "#f2f2f7",
  fg: "#1c1c1e",
  muted: "#5c5c61",
  chip: "rgba(0,0,0,0.07)",
  chipStrong: "rgba(0,0,0,0.12)",
  card: "#ffffff",
  cardFg: "#1c1c1e",
  cardMuted: "#5c5c61",
  separator: "rgba(0,0,0,0.06)",
  accent: "#ff6652",
  accentStrong: "#d8432b",
  origin: "#c64735",
  danger: "#b3261e",
  ok: "#2b8a3e",
};

const dark: Theme = {
  dark: true,
  bg: "#000000",
  fg: "#f2f2f7",
  muted: "#a1a1a8",
  chip: "rgba(255,255,255,0.1)",
  chipStrong: "rgba(255,255,255,0.16)",
  card: "#1c1c1e",
  cardFg: "#f2f2f7",
  cardMuted: "#a1a1a8",
  separator: "rgba(255,255,255,0.08)",
  accent: "#ff6652",
  accentStrong: "#e0513a",
  origin: "#ff9585",
  danger: "#ff6b63",
  ok: "#4fbf63",
};

export function useTheme(): Theme {
  return useColorScheme() === "dark" ? dark : light;
}

export const radius = { card: 16, button: 26, chip: 7 };
