import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

export type ThemePreference = "system" | "light" | "dark";

type ThemeContextValue = {
  themePreference: ThemePreference;
  setThemePreference: (preference: ThemePreference) => void;
};

const themeStorageKey = "exagent.theme";

const defaultThemeContext: ThemeContextValue = {
  themePreference: "system",
  setThemePreference: () => {}
};

const ThemeContext = createContext<ThemeContextValue>(defaultThemeContext);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [themePreference, setThemePreferenceState] = useState<ThemePreference>(() => readStoredThemePreference());

  useEffect(() => {
    if (themePreference === "system") {
      delete document.documentElement.dataset.theme;
    } else {
      document.documentElement.dataset.theme = themePreference;
    }
    window.localStorage.setItem(themeStorageKey, themePreference);
  }, [themePreference]);

  const value = useMemo<ThemeContextValue>(
    () => ({
      themePreference,
      setThemePreference: setThemePreferenceState
    }),
    [themePreference]
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useThemePreference() {
  return useContext(ThemeContext);
}

function readStoredThemePreference(): ThemePreference {
  if (typeof window === "undefined") {
    return "system";
  }
  const stored = window.localStorage.getItem(themeStorageKey);
  return stored === "light" || stored === "dark" || stored === "system" ? stored : "system";
}
