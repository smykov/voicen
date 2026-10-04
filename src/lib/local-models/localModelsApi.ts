// The local-model IPC as the settings window sees it (spec 002 contracts/ipc.md, T-045).
//
// One IPC module per contract (settings-ui.md › C): this module holds the only
// invoke/listen calls of spec 002 and the one TS declaration of its wire (the serde
// forms of voicen_core::local_models::service, pinned by
// e2e/fixtures/local-models-wire.json); the e2e mock re-exports these types. Nothing
// here decides a row's state: the rows come from `local_models_list` and the
// `local-model://` events only (I2, models.ts).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
// Relative, not `$lib`: e2e/support/tauriMock.ts imports these wire types too.
import type { MessageId } from "../i18n";

/** `voicen_core::local_models::catalog::ModelId::as_str`, catalog order. */
export type ModelId = "tiny" | "base" | "small" | "medium-q5_0" | "large-v3-turbo-q5_0";

/**
 * Why a download failed or was refused (`ReasonView`). `messageKey` is a Rust
 * `MessageId` (`download.*`), typed `MessageId` at this boundary like
 * `FormError.message`. `params` is present only when there are any; every value is a
 * string. `needed` is a byte count, shown only through `reasonArgs` (models.ts).
 */
export interface FailureReason {
  code: string;
  messageKey: MessageId;
  params?: Record<string, string>;
}

/** `LocalModelState` on the wire. `failed` stays until a retry; a restart lists it as not_downloaded. */
export type ModelState =
  | { kind: "not_downloaded" }
  | { kind: "downloading"; received: number; total: number }
  | { kind: "downloaded" }
  | { kind: "failed"; reason: FailureReason };

/** One row of `local_models_list` (always five, catalog order). */
export interface LocalModelView {
  id: ModelId;
  /** `local_model.name.<id>`, a Rust `MessageId`. */
  nameKey: MessageId;
  sizeBytes: number;
  /** True only for `small`. */
  recommended: boolean;
  state: ModelState;
  /** Always false until T-017's residency; not rendered. */
  loaded: boolean;
}

/** `local-model://progress` payload. */
export interface LocalModelProgress {
  id: ModelId;
  received: number;
  total: number;
}

/** `local-model://state` payload: every state transition. */
export interface LocalModelStateChange {
  id: ModelId;
  state: ModelState;
}

/** `voicen_core::local_models::service::PROGRESS_EVENT`. */
const PROGRESS_EVENT = "local-model://progress";
/** `voicen_core::local_models::service::STATE_EVENT`. */
const STATE_EVENT = "local-model://state";

/**
 * A rejection of a local-model command as the contract's `FailureReason` (an object
 * with a string `code` and `messageKey`, and string `params` if any), or null for any
 * other rejection (the caller shows `error.ipc_unavailable`, never the rejection).
 * `messageKey` is taken as the Rust `MessageId` core sends, as every wire id is.
 */
export function asFailureReason(value: unknown): FailureReason | null {
  if (typeof value !== "object" || value === null) return null;
  const candidate = value as Record<string, unknown>;
  if (typeof candidate.code !== "string" || typeof candidate.messageKey !== "string") return null;
  const params = candidate.params;
  if (params !== undefined) {
    if (typeof params !== "object" || params === null) return null;
    if (!Object.values(params).every((param) => typeof param === "string")) return null;
  }
  return value as FailureReason;
}

/** `local_models_list`: the five catalog models with their states. */
export function listLocalModels(): Promise<LocalModelView[]> {
  return invoke<LocalModelView[]>("local_models_list");
}

/**
 * `local_model_download { id }`: starts the download and emits nothing itself;
 * rejects with a `FailureReason` when refused (no state changes then).
 */
export function downloadLocalModel(id: ModelId): Promise<void> {
  return invoke<void>("local_model_download", { id });
}

/** `local_model_cancel_download { id }`: true if a running download was cancelled. */
export function cancelLocalModelDownload(id: ModelId): Promise<boolean> {
  return invoke<boolean>("local_model_cancel_download", { id });
}

/** `local-model://progress`, emitted after the listed state is updated. */
export function onLocalModelProgress(handler: (progress: LocalModelProgress) => void): Promise<UnlistenFn> {
  return listen<LocalModelProgress>(PROGRESS_EVENT, (event) => handler(event.payload));
}

/** `local-model://state`, emitted after the listed state is updated. */
export function onLocalModelState(handler: (change: LocalModelStateChange) => void): Promise<UnlistenFn> {
  return listen<LocalModelStateChange>(STATE_EVENT, (event) => handler(event.payload));
}
