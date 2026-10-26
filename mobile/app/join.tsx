// First run: scan the QR the desktop shows under 설정 → 공유 → 휴대폰 추가.

import { CameraView, useCameraPermissions } from "expo-camera";
import * as Clipboard from "expo-clipboard";
import { Redirect, useRouter } from "expo-router";
import React, { useCallback, useRef, useState } from "react";
import { ActivityIndicator, Image, Linking, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useApp } from "@/app-context";
import { parseJoinLink } from "@/protocol/join";
import { Button } from "@/ui/Button";

export default function Join() {
  const { ready, session, connect } = useApp();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  const [permission, requestPermission] = useCameraPermissions();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const handled = useRef(false);

  const join = useCallback(
    async (link: string) => {
      if (handled.current) return;
      handled.current = true;
      setBusy(true);
      setError(null);
      try {
        await connect(link);
        router.replace("/");
      } catch (e) {
        setError(e instanceof Error ? e.message : "연결하지 못했어요.");
        handled.current = false;
      } finally {
        setBusy(false);
      }
    },
    [connect, router],
  );

  const fromClipboard = useCallback(async () => {
    const text = await Clipboard.getStringAsync();
    if (!parseJoinLink(text)) {
      setError("클립보드에 Pinchy 연결 링크가 없어요. 데스크톱 QR 아래의 링크를 복사해 오세요.");
      return;
    }
    await join(text);
  }, [join]);

  if (ready && session) return <Redirect href="/" />;

  return (
    <View style={[styles.screen, { paddingTop: insets.top + 24, paddingBottom: insets.bottom + 24 }]}>
      <View style={styles.brand}>
        <Image source={require("../assets/icon-small.png")} style={{ width: 56, height: 56 }} accessibilityIgnoresInvertColors />
        <Text style={styles.title}>Pinchy</Text>
      </View>

      <View style={styles.finder}>
        {permission?.granted ? (
          <CameraView
            style={styles.camera}
            facing="back"
            barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
            onBarcodeScanned={busy ? undefined : (result) => void join(result.data)}
          />
        ) : (
          <View style={[styles.camera, styles.cameraOff]}>
            <Text style={styles.cameraOffText}>{permission === null ? "" : permission.canAskAgain ? "카메라를 허용하면 QR을 찍을 수 있어요" : "설정에서 Pinchy의 카메라 접근을 허용해 주세요"}</Text>
            {permission && (
              <Pressable
                onPress={() => (permission.canAskAgain ? void requestPermission() : void Linking.openSettings())}
                style={styles.allow}
                accessibilityRole="button"
              >
                <Text style={styles.allowText}>{permission.canAskAgain ? "카메라 허용" : "설정 열기"}</Text>
              </Pressable>
            )}
          </View>
        )}
        <Corner style={{ top: 0, left: 0 }} />
        <Corner style={{ top: 0, right: 0, transform: [{ rotate: "90deg" }] }} />
        <Corner style={{ bottom: 0, right: 0, transform: [{ rotate: "180deg" }] }} />
        <Corner style={{ bottom: 0, left: 0, transform: [{ rotate: "270deg" }] }} />
        {busy ? (
          <View style={styles.busy}>
            <ActivityIndicator color="#fff" size="large" />
          </View>
        ) : null}
      </View>

      <View style={styles.copy}>
        <Text style={styles.heading}>데스크톱의 QR을 찍어 주세요</Text>
        <Text style={styles.body}>
          컴퓨터에서 Pinchy 설정 → 공유 → <Text style={styles.strong}>휴대폰 추가</Text>를 누르면 QR이 나와요. 코드는 10분 동안 유효해요.
        </Text>
        {error ? <Text style={styles.error}>{error}</Text> : null}
      </View>

      <View style={styles.bottom}>
        <Button title="복사한 링크로 연결" kind="secondary" onPress={() => void fromClipboard()} busy={busy} style={styles.secondary} />
        <Text style={styles.hint}>공유 키는 이 폰의 보안 저장소에만 저장돼요</Text>
      </View>
    </View>
  );
}

function Corner({ style }: { style: object }) {
  return <View pointerEvents="none" style={[styles.corner, style]} />;
}

const SIDE = 262;

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: "#121214", alignItems: "center" },
  brand: { alignItems: "center", gap: 10, marginTop: 24 },
  title: { color: "#f2f2f7", fontSize: 24, fontWeight: "800", letterSpacing: -0.6 },
  finder: { width: SIDE, height: SIDE, marginTop: 48, position: "relative" },
  camera: { width: SIDE, height: SIDE, borderRadius: 22, overflow: "hidden", backgroundColor: "#2a2a2e" },
  cameraOff: { alignItems: "center", justifyContent: "center", padding: 24, gap: 14 },
  cameraOffText: { color: "#b8b8bf", fontSize: 14, textAlign: "center", lineHeight: 20 },
  allow: { height: 40, paddingHorizontal: 18, borderRadius: 20, backgroundColor: "#ff6652", alignItems: "center", justifyContent: "center" },
  allowText: { color: "#1c1c1e", fontSize: 14, fontWeight: "700" },
  corner: { position: "absolute", width: 48, height: 48, borderColor: "#ff6652", borderTopWidth: 5, borderLeftWidth: 5, borderTopLeftRadius: 22 },
  busy: { position: "absolute", top: 0, left: 0, right: 0, bottom: 0, alignItems: "center", justifyContent: "center", backgroundColor: "rgba(0,0,0,0.45)", borderRadius: 22 },
  copy: { marginTop: 40, paddingHorizontal: 36, alignItems: "center", gap: 10 },
  heading: { color: "#f2f2f7", fontSize: 18, fontWeight: "700" },
  body: { color: "#b8b8bf", fontSize: 14, lineHeight: 22, textAlign: "center" },
  strong: { color: "#f2f2f7", fontWeight: "600" },
  error: { color: "#ff9585", fontSize: 13, lineHeight: 19, textAlign: "center", marginTop: 4 },
  bottom: { marginTop: "auto", width: "100%", paddingHorizontal: 20, gap: 10 },
  secondary: { backgroundColor: "rgba(255,255,255,0.1)", height: 50, borderRadius: 25 },
  hint: { color: "#8a8a91", fontSize: 12, textAlign: "center" },
});
