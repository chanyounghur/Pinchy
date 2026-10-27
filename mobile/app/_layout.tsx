import { ShareIntentProvider } from "expo-share-intent";
import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import React from "react";
import { GestureHandlerRootView } from "react-native-gesture-handler";
import { SafeAreaProvider } from "react-native-safe-area-context";
import { AppProvider } from "@/app-context";
import { useTheme } from "@/ui/theme";
import { ToastProvider } from "@/ui/Toast";

export default function RootLayout() {
  const t = useTheme();
  return (
    <GestureHandlerRootView style={{ flex: 1 }}>
      <SafeAreaProvider>
        <ShareIntentProvider options={{ resetOnBackground: false }}>
          <AppProvider>
            <ToastProvider>
              <StatusBar style={t.dark ? "light" : "dark"} />
              <Stack
                screenOptions={{
                  headerShown: false,
                  contentStyle: { backgroundColor: t.bg },
                }}
              >
                <Stack.Screen name="index" />
                <Stack.Screen name="join" options={{ gestureEnabled: false }} />
                <Stack.Screen name="item/[id]" />
                <Stack.Screen name="settings" />
                <Stack.Screen
                  name="add"
                  options={{
                    presentation: "formSheet",
                    sheetAllowedDetents: "fitToContents",
                    sheetGrabberVisible: true,
                    sheetCornerRadius: 20,
                  }}
                />
                <Stack.Screen name="upload" options={{ presentation: "modal" }} />
              </Stack>
            </ToastProvider>
          </AppProvider>
        </ShareIntentProvider>
      </SafeAreaProvider>
    </GestureHandlerRootView>
  );
}
