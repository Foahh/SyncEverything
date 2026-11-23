import { useEffect, useState } from "react";

const THEME_STORAGE_KEY = "relay.theme";
export type ThemeScheme = "light" | "dark";

function readStoredTheme(): ThemeScheme | null {
  if (typeof window === "undefined") return null;
  try {
    const saved = window.localStorage.getItem(THEME_STORAGE_KEY);
    return saved === "light" || saved === "dark" ? saved : null;
  } catch {
    return null;
  }
}

function readSystemTheme(): ThemeScheme {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function useRelayTheme() {
  const [storedTheme, setStoredTheme] = useState<ThemeScheme | null>(readStoredTheme);
  const [systemTheme, setSystemTheme] = useState<ThemeScheme>(readSystemTheme);
  const theme: ThemeScheme = storedTheme ?? systemTheme;

  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle("dark", theme === "dark");
    root.dataset.theme = theme;
    const meta = document.querySelector<HTMLMetaElement>('meta[name="color-scheme"]');
    if (meta) {
      meta.content = storedTheme ?? "light dark";
    }
  }, [theme, storedTheme]);

  useEffect(() => {
    if (typeof window === "undefined") return;
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    const handleMediaChange = (event: MediaQueryListEvent) => {
      setSystemTheme(event.matches ? "dark" : "light");
    };
    const handleStorage = (event: StorageEvent) => {
      if (event.key === THEME_STORAGE_KEY) {
        setStoredTheme(readStoredTheme());
      }
    };
    media?.addEventListener("change", handleMediaChange);
    window.addEventListener("storage", handleStorage);
    return () => {
      media?.removeEventListener("change", handleMediaChange);
      window.removeEventListener("storage", handleStorage);
    };
  }, []);

  const toggleTheme = () => {
    const target: ThemeScheme = theme === "light" ? "dark" : "light";
    const currentSystem = readSystemTheme();
    if (target === currentSystem) {
      setStoredTheme(null);
      try {
        window.localStorage.removeItem(THEME_STORAGE_KEY);
      } catch {
        // Ignore storage errors in restricted environments
      }
    } else {
      setStoredTheme(target);
      try {
        window.localStorage.setItem(THEME_STORAGE_KEY, target);
      } catch {
        // Ignore storage errors in restricted environments
      }
    }
  };

  return { theme, toggleTheme };
}
