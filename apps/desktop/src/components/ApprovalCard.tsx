import { ShieldAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { submitApproval } from "@/stores/workbenchStore";
import type { TranscriptMessage } from "@/types";

export function ApprovalCard({ message }: { message: TranscriptMessage }) {
  return (
    <article className="rounded-lg border border-warning/40 bg-surface-1 px-4 py-3">
      <div className="flex items-start gap-3">
        <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-3">
            <h3 className="text-sm font-semibold text-ink">{message.toolName ?? "Approval requested"}</h3>
            <span className="font-mono text-xs text-subtle">{message.timestamp}</span>
          </div>
          <p className="mt-2 whitespace-pre-wrap text-sm leading-6 text-muted">{message.body}</p>
          <div className="mt-3 flex items-center gap-2">
            <Button type="button" size="sm" onClick={() => void submitApproval(message, "approved")}>
              Approve
            </Button>
            <Button type="button" size="sm" variant="danger" onClick={() => void submitApproval(message, "denied")}>
              Deny
            </Button>
          </div>
        </div>
      </div>
    </article>
  );
}
