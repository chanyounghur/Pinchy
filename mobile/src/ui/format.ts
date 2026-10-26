export function relativeTime(ms: number, now = Date.now()): string {
  const diff = now - ms;
  if (diff < 60_000) return "방금";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}분 전`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}시간 전`;
  const then = new Date(ms);
  const today = new Date(now);
  const dayStart = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((dayStart(today) - dayStart(then)) / 86_400_000);
  if (days === 1) return "어제";
  if (days < 7) return `${days}일 전`;
  return `${then.getMonth() + 1}월 ${then.getDate()}일`;
}

export function fullTime(ms: number): string {
  const d = new Date(ms);
  const h = d.getHours();
  const period = h < 12 ? "오전" : "오후";
  const hour = h % 12 === 0 ? 12 : h % 12;
  return `${d.getFullYear()}년 ${d.getMonth() + 1}월 ${d.getDate()}일 ${period} ${hour}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export function kindLabel(kind: string): string {
  switch (kind) {
    case "text":
      return "텍스트";
    case "link":
      return "링크";
    case "image":
      return "이미지";
    case "files":
      return "파일";
    default:
      return kind;
  }
}

export function domainOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/// A stable card color for an app name. The desktop uses the app icon's
/// color; the phone only knows the name.
const PALETTE = ["#3b5bdb", "#2b8a3e", "#6741d9", "#c2410c", "#0b7285", "#a61e4d", "#5c5f66", "#1864ab", "#862e9c", "#9c4221"];

export function appColor(app: string | null): string {
  if (!app) return "#5c5f66";
  let h = 0;
  for (let i = 0; i < app.length; i++) h = (h * 31 + app.charCodeAt(i)) >>> 0;
  return PALETTE[h % PALETTE.length]!;
}

export function initial(app: string | null): string {
  const t = (app ?? "").trim();
  return t ? t[0]!.toUpperCase() : "?";
}
