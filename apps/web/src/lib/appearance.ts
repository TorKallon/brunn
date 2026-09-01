export type Appearance = "dark" | "light";

export const APPEARANCE_STORAGE_KEY = "brunn.appearance.v1";
export const DEFAULT_APPEARANCE: Appearance = "dark";

export function readAppearance(
  storage: Pick<Storage, "getItem"> | undefined = browserStorage(),
): Appearance {
  try {
    const saved = storage?.getItem(APPEARANCE_STORAGE_KEY);
    return saved === "light" || saved === "dark" ? saved : DEFAULT_APPEARANCE;
  } catch {
    return DEFAULT_APPEARANCE;
  }
}

export function applyAppearance(
  appearance: Appearance,
  root: HTMLElement | undefined = browserRoot(),
) {
  if (!root) return;
  if (appearance === "light") {
    root.dataset.theme = "light";
  } else {
    delete root.dataset.theme;
  }
  root.style.colorScheme = appearance;

  const themeColor = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
  if (themeColor) {
    themeColor.content = appearance === "dark" ? "#06152c" : "#f5f6fa";
  }
}

export function saveAppearance(
  appearance: Appearance,
  storage: Pick<Storage, "setItem"> | undefined = browserStorage(),
) {
  try {
    storage?.setItem(APPEARANCE_STORAGE_KEY, appearance);
  } catch {
    // Appearance still applies for this page even when storage is unavailable.
  }
  applyAppearance(appearance);
}

export function initializeAppearance(): Appearance {
  const appearance = readAppearance();
  applyAppearance(appearance);
  return appearance;
}

function browserStorage(): Storage | undefined {
  return typeof window === "undefined" ? undefined : window.localStorage;
}

function browserRoot(): HTMLElement | undefined {
  return typeof document === "undefined" ? undefined : document.documentElement;
}
