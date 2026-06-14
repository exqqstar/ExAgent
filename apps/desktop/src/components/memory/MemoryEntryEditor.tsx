import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import type { MemoryEntryView, MemoryHitView, MemoryUpdateAction } from "@/lib/api/memory";

export type MemoryEntryEditorMode = "candidate_promote" | "edit" | "supersede";

export interface MemoryEntryEditorSubmit {
  action: MemoryUpdateAction;
  kind: string;
  title: string;
  content: string;
  files: string[];
  concepts: string[];
  pinned: boolean;
}

export function MemoryEntryEditor({
  entry,
  mode,
  open,
  saving = false,
  onOpenChange,
  onSubmit
}: {
  entry: MemoryEntryView | MemoryHitView | null;
  mode: MemoryEntryEditorMode;
  open: boolean;
  saving?: boolean;
  onOpenChange: (open: boolean) => void;
  onSubmit: (input: MemoryEntryEditorSubmit) => Promise<void> | void;
}) {
  const [kind, setKind] = useState("");
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [files, setFiles] = useState("");
  const [concepts, setConcepts] = useState("");
  const [pinned, setPinned] = useState(false);

  useEffect(() => {
    if (!entry || !open) {
      return;
    }

    setKind(entry.kind);
    setTitle(entry.title);
    setBody(entry.body);
    setFiles(entry.files.map(filePath).join("\n"));
    setConcepts("concepts" in entry ? entry.concepts.join(", ") : "");
    setPinned("pinned" in entry ? Boolean(entry.pinned) : false);
  }, [entry, open]);

  if (!entry) {
    return null;
  }

  const dialogTitle = mode === "candidate_promote" ? "Edit and promote memory" : mode === "edit" ? "Edit memory" : "Supersede memory";
  const submitLabel = mode === "candidate_promote" ? "Promote" : "Save";

  async function submit() {
    await onSubmit({
      action: "supersede",
      kind: kind.trim(),
      title: title.trim(),
      content: body.trim(),
      files: lines(files),
      concepts: concepts
        .split(",")
        .map((concept) => concept.trim())
        .filter(Boolean),
      pinned
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="gap-3">
        <DialogHeader>
          <DialogTitle>{dialogTitle}</DialogTitle>
          <DialogDescription>{entry.id}</DialogDescription>
        </DialogHeader>

        <div className="grid gap-3">
          <label className="grid gap-1.5">
            <span className="type-label-sm text-muted">Kind</span>
            <Input value={kind} onChange={(event) => setKind(event.target.value)} />
          </label>
          <label className="grid gap-1.5">
            <span className="type-label-sm text-muted">Title</span>
            <Input value={title} onChange={(event) => setTitle(event.target.value)} />
          </label>
          <label className="grid gap-1.5">
            <span className="type-label-sm text-muted">Content</span>
            <Textarea value={body} onChange={(event) => setBody(event.target.value)} />
          </label>
          <label className="grid gap-1.5">
            <span className="type-label-sm text-muted">Files</span>
            <Textarea className="type-code-sm min-h-[72px]" value={files} onChange={(event) => setFiles(event.target.value)} />
          </label>
          <label className="grid gap-1.5">
            <span className="type-label-sm text-muted">Concepts</span>
            <Input value={concepts} onChange={(event) => setConcepts(event.target.value)} />
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              className="h-4 w-4 accent-primary"
              checked={pinned}
              onChange={(event) => setPinned(event.target.checked)}
            />
            <span className="type-label-sm text-muted">Pinned</span>
          </label>
        </div>

        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={submit} disabled={saving || title.trim().length === 0 || body.trim().length === 0}>
            {submitLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function filePath(file: string | { path: string }) {
  return typeof file === "string" ? file : file.path;
}

function lines(value: string) {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}
