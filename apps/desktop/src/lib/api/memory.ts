export type MemoryScope = "project" | "global";
export type MemorySource = "entry" | string;

export interface MemoryFileRef {
  path: string;
  stale?: boolean;
}

export interface MemoryHitView {
  id: string;
  source: MemorySource;
  scope: MemoryScope | string;
  kind: string;
  title: string;
  body: string;
  files: Array<string | MemoryFileRef>;
  concepts: string[];
  confidence: number;
  stale: boolean;
  quarantined: boolean;
  rank: number;
  pinned?: boolean;
  status?: string;
  use_count?: number;
  supersedes_id?: string | null;
}

export interface MemoryEntryView {
  id: string;
  scope: MemoryScope | string;
  kind: string;
  title: string;
  body: string;
  files: Array<string | MemoryFileRef>;
  concepts: string[];
  confidence: number;
  pinned: boolean;
  status: string;
  stale: boolean;
  quarantined: boolean;
  inactive_reason?: string | null;
  supersedes_id?: string | null;
  created_at_ms: number;
  updated_at_ms: number;
  use_count?: number;
  quarantine_reason?: string | null;
}

export interface MemoryAuditEventView {
  id: string;
  memory_id: string;
  action: string;
  actor: string;
  created_at_ms: number;
  details: unknown;
}

export interface MemorySearchResponse {
  hits: MemoryHitView[];
}

export interface MemorySaveInput {
  kind: string;
  title: string;
  content: string;
  files?: string[];
  concepts?: string[];
  pinned?: boolean;
}

export interface MemorySaveResponse {
  entry: MemoryEntryView;
}

export type MemoryUpdateAction = "pin" | "unpin" | "archive" | "unarchive" | "reject" | "supersede";

export interface MemoryUpdateResponse {
  entry: MemoryEntryView;
}

export interface MemoryForgetResponse {
  forgotten?: boolean;
  entry_id?: string;
}

export interface MemoryAuditResponse {
  events: MemoryAuditEventView[];
}

export interface MemoryListCandidatesResponse {
  candidates: MemoryEntryView[];
}

export interface MemoryListArchivedResponse {
  archived: MemoryEntryView[];
}

export interface MemoryPromoteResponse {
  entry: MemoryEntryView;
}

async function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function ensureTauriRuntimeForMemoryWrite() {
  if (!isTauriRuntime()) {
    throw new Error("Memory writes require the desktop runtime");
  }
}

export async function memorySearch(
  projectId: string,
  scope: MemoryScope = "project",
  query = "",
  limit = 50
): Promise<MemorySearchResponse> {
  if (!isTauriRuntime()) {
    return { hits: [] };
  }
  return invokeCommand<MemorySearchResponse>("memory_search", { projectId, scope, query, limit });
}

export async function memorySave(
  projectId: string,
  scope: MemoryScope,
  input: MemorySaveInput
): Promise<MemorySaveResponse> {
  ensureTauriRuntimeForMemoryWrite();
  return invokeCommand<MemorySaveResponse>("memory_save", { projectId, scope, input });
}

export async function memoryUpdate(
  projectId: string,
  entryId: string,
  action: MemoryUpdateAction,
  scope: MemoryScope,
  kind?: string,
  title?: string,
  content?: string,
  files?: string[],
  concepts?: string[],
  pinned?: boolean
): Promise<MemoryUpdateResponse> {
  ensureTauriRuntimeForMemoryWrite();
  return invokeCommand<MemoryUpdateResponse>("memory_update", {
    projectId,
    entryId,
    action,
    scope,
    kind,
    title,
    content,
    files,
    concepts,
    pinned
  });
}

export async function memoryForget(
  projectId: string,
  entryId: string,
  scope?: MemoryScope
): Promise<MemoryForgetResponse> {
  ensureTauriRuntimeForMemoryWrite();
  return invokeCommand<MemoryForgetResponse>("memory_forget", { projectId, entryId, scope });
}

export async function memoryAudit(
  projectId: string,
  scope: MemoryScope = "project",
  entryId?: string,
  limit = 50
): Promise<MemoryAuditResponse> {
  if (!isTauriRuntime()) {
    return { events: [] };
  }
  return invokeCommand<MemoryAuditResponse>("memory_audit", { projectId, scope, entryId, limit });
}

export async function memoryListCandidates(
  projectId: string,
  scope: MemoryScope = "project",
  query = "",
  limit = 50
): Promise<MemoryListCandidatesResponse> {
  if (!isTauriRuntime()) {
    return { candidates: [] };
  }
  return invokeCommand<MemoryListCandidatesResponse>("memory_list_candidates", { projectId, scope, query, limit });
}

export async function memoryListArchived(
  projectId: string,
  scope: MemoryScope = "project",
  query = "",
  limit = 50
): Promise<MemoryListArchivedResponse> {
  if (!isTauriRuntime()) {
    return { archived: [] };
  }
  return invokeCommand<MemoryListArchivedResponse>("memory_list_archived", { projectId, scope, query, limit });
}

export async function memoryPromote(
  projectId: string,
  entryId: string,
  scope: MemoryScope = "project",
  allowQuarantinedOverride = false
): Promise<MemoryPromoteResponse> {
  ensureTauriRuntimeForMemoryWrite();
  return invokeCommand<MemoryPromoteResponse>("memory_promote", { projectId, entryId, scope, allowQuarantinedOverride });
}
