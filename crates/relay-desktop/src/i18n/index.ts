import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en.json";
import zhCN from "./locales/zh-CN.json";

export type RelayLocale = "en" | "zh-CN";
export type TranslationSchema = typeof en;

// Compile-time parity check: ensures zh-CN.json never drifts from en.json keys
const zhCNVerified: TranslationSchema = zhCN;

export const resources = {
  en: { translation: en },
  "zh-CN": { translation: zhCNVerified },
} as const;

const STORAGE_KEY = "relay.locale";

function detectInitialLanguage(): RelayLocale {
  if (typeof window === "undefined") return "en";
  const saved = window.localStorage.getItem(STORAGE_KEY);
  if (saved === "en" || saved === "zh-CN") return saved;
  return navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en";
}

const initialLocale = detectInitialLanguage();
if (typeof document !== "undefined") {
  document.documentElement.lang = initialLocale;
}

void i18n.use(initReactI18next).init({
  resources,
  lng: initialLocale,
  fallbackLng: "en",
  defaultNS: "translation",
  returnNull: false,
  interpolation: {
    escapeValue: false,
  },
});

export function setRelayLocale(locale: RelayLocale): void {
  void i18n.changeLanguage(locale);
  if (typeof window !== "undefined") {
    window.localStorage.setItem(STORAGE_KEY, locale);
  }
  if (typeof document !== "undefined") {
    document.documentElement.lang = locale;
  }
}

export default i18n;
