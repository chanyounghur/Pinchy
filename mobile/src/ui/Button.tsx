import React from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, type StyleProp, type ViewStyle } from "react-native";
import { radius, useTheme } from "./theme";

export function Button({
  title,
  onPress,
  kind = "primary",
  busy = false,
  disabled = false,
  style,
}: {
  title: string;
  onPress?: () => void;
  kind?: "primary" | "secondary" | "danger";
  busy?: boolean;
  disabled?: boolean;
  style?: StyleProp<ViewStyle>;
}) {
  const t = useTheme();
  const bg = kind === "primary" ? t.accentStrong : t.chip;
  const fg = kind === "primary" ? "#fff" : kind === "danger" ? t.danger : t.fg;
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled || busy}
      accessibilityRole="button"
      style={({ pressed }) => [styles.button, { backgroundColor: bg, opacity: disabled ? 0.5 : pressed ? 0.8 : 1 }, style]}
    >
      {busy ? <ActivityIndicator color={fg} /> : <Text style={[styles.title, { color: fg }]}>{title}</Text>}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: { height: 52, borderRadius: radius.button, alignItems: "center", justifyContent: "center", paddingHorizontal: 22 },
  title: { fontSize: 16, fontWeight: "700" },
});
