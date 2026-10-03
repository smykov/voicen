import { invoke } from "@tauri-apps/api/core";
import { t } from "$lib/i18n";

/** Version and commit of the running build (FR-18), as returned by the `get_build_info` command. */
export interface BuildInfo {
  version: string;
  commit: string;
}

export function formatBuildInfo(info: BuildInfo): string {
  return t("app.build_info", { version: info.version, commit: info.commit });
}

export function loadBuildInfo(): Promise<BuildInfo> {
  return invoke<BuildInfo>("get_build_info");
}
