import { CircleAlert } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { submitUserInput } from "@/stores/workbenchStore";
import type { TranscriptMessage } from "@/types";

export function QuestionCard({ message, readOnly = false }: { message: TranscriptMessage; readOnly?: boolean }) {
  const questions = message.questions ?? [];
  const [answers, setAnswers] = useState<string[]>(() => questions.map(() => ""));
  const pending = message.toolStatus === "waiting_user_input";

  const submit = (dismissed: boolean) => {
    const payload = dismissed ? [] : answers.map((answer) => (answer.trim() ? [answer.trim()] : []));
    void submitUserInput(message, payload, dismissed);
  };

  return (
    <article className="message-card rounded-lg border border-warning/40 px-4 py-3">
      <div className="flex items-start gap-3">
        <CircleAlert className="mt-0.5 h-4 w-4 shrink-0 text-warning" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-3">
            <h3 className="type-title-sm text-ink">Question for you</h3>
            <span className="type-code-sm text-subtle">{message.timestamp}</span>
          </div>
          <div className="mt-3 space-y-3">
            {questions.length > 0 ? (
              questions.map((question, index) => (
                <label key={`${message.id}-${index}`} className="block">
                  <span className="type-body-md block text-ink">{question.question}</span>
                  {question.options?.length ? (
                    <span className="type-body-sm mt-1 block text-subtle">
                      {question.options.map((option) => option.label).join(", ")}
                    </span>
                  ) : null}
                  {readOnly || !pending ? null : (
                    <input
                      className="mt-2 w-full rounded-md border border-border bg-surface px-3 py-2 type-body-md text-ink outline-none focus:border-accent"
                      value={answers[index] ?? ""}
                      onChange={(event) => {
                        const next = [...answers];
                        next[index] = event.target.value;
                        setAnswers(next);
                      }}
                    />
                  )}
                </label>
              ))
            ) : (
              <p className="type-body-md whitespace-pre-wrap text-muted">{message.body}</p>
            )}
          </div>
          {readOnly || !pending ? null : (
            <div className="mt-3 flex items-center gap-2">
              <Button type="button" size="sm" onClick={() => submit(false)}>
                Submit
              </Button>
              <Button type="button" size="sm" variant="ghost" onClick={() => submit(true)}>
                Dismiss
              </Button>
            </div>
          )}
        </div>
      </div>
    </article>
  );
}
