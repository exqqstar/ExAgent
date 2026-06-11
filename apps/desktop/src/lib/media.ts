import { convertFileSrc } from "@tauri-apps/api/core";

export function localFileAssetSrc(path: string) {
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    return convertFileSrc(path);
  }
  return `file://${path}`;
}

export function fileBaseName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}
