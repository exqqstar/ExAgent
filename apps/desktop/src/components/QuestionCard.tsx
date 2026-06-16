import { CircleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { submitUserInput } from "@/stores/workbenchStore";
import type { TranscriptMessage } from "@/types";

export function QuestionCard({ message, readOnly = false }: { message: TranscriptMessage; readOnly?: boolean }) {
  const questions = message.questions ?? [];
  const [answers, setAnswers] = useState<string[][]>(() => questions.map(() => []));
  const pending = message.toolStatus === "waiting_user_input";

  useEffect(() => {
    setAnswers(questions.map(() => []));
  }, [message.id, questions.length]);

  const submit = (dismissed: boolean) => {
    const payload = dismissed
      ? []
      : answers.map((answer) => answer.map((value) => value.trim()).filter(Boolean));
    void submitUserInput(message, payload, dismissed);
  };

  function setTextAnswer(index: number, value: string) {
    setAnswers((current) => {
      const next = [...current];
      next[index] = value.trim() ? [value] : [];
      return next;
    });
  }

  function setSingleOptionAnswer(index: number, value: string) {
    setAnswers((current) => {
      const next = [...current];
      next[index] = [value];
      return next;
    });
  }

  function toggleMultiOptionAnswer(index: number, value: string, checked: boolean) {
    setAnswers((current) => {
      const next = [...current];
      const currentAnswers = new Set(next[index] ?? []);
      if (checked) {
        currentAnswers.add(value);
      } else {
        currentAnswers.delete(value);
      }
      next[index] = Array.from(currentAnswers);
      return next;
    });
  }

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
                <div key={`${message.id}-${index}`} className="block">
                  <p className="type-body-md text-ink">{question.question}</p>
                  {question.options?.length ? (
                    <div
                      className="mt-2 grid gap-2"
                      role={question.multi_select ? "group" : "radiogroup"}
                      aria-label={question.question}
                    >
                      {question.options.map((option) => {
                        const selected = (answers[index] ?? []).includes(option.label);
                        const inputType = question.multi_select ? "checkbox" : "radio";
                        return (
                          <label
                            key={option.label}
                            className="type-body-md flex items-start gap-2 rounded-lg border border-border bg-surface-1 px-3 py-2 text-ink"
                          >
                            <input
                              type={inputType}
                              name={`${message.id}-${index}`}
                              checked={selected}
                              disabled={readOnly || !pending}
                              onChange={(event) => {
                                if (question.multi_select) {
                                  toggleMultiOptionAnswer(index, option.label, event.target.checked);
                                } else {
                                  setSingleOptionAnswer(index, option.label);
                                }
                              }}
                            />
                            <span>
                              <span className="block">{option.label}</span>
                              {option.description ? (
                                <span className="type-body-sm mt-1 block text-subtle">{option.description}</span>
                              ) : null}
                            </span>
                          </label>
                        );
                      })}
                    </div>
                  ) : readOnly || !pending ? null : (
                    <input
                      className="control-field mt-2 w-full rounded-lg border border-border bg-surface-1 px-3 py-2 type-body-md text-ink outline-none"
                      value={answers[index]?.[0] ?? ""}
                      onChange={(event) => setTextAnswer(index, event.target.value)}
                    />
                  )}
                </div>
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
