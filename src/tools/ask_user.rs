use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::policy::{QuestionOption, QuestionPrompt};
use crate::registry::ToolContext;
use crate::session::ApprovalId;
use crate::tools::{
    Tool, ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolRuntimeEffect, ToolSpec,
};
use crate::types::{ToolCall, ToolResult, ToolStatus};

const MAX_QUESTIONS: usize = 4;
const MAX_OPTIONS_PER_QUESTION: usize = 6;
const MAX_HEADER_CHARS: usize = 24;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskUserArgs {
    pub questions: Option<Vec<QuestionPrompt>>,
    pub request_id: Option<String>,
    pub decision: Option<String>,
}

pub struct AskUserTool;

#[async_trait]
impl ToolHandler for AskUserTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "ask_user",
            "Ask the user one or more questions and wait for their answers; use only when a decision genuinely belongs to the user",
            serde_json::to_value(schemars::schema_for!(AskUserArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            mutating: false,
            requires_approval: false,
            parallel_safe: false,
        }
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<AskUserArgs>(call.arguments) {
            Ok(args) => args,
            Err(err) => return ask_user_error(call.id, call.name, err.to_string()),
        };

        match handle_ask_user(&args, ctx).await {
            Ok(result) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: result.status,
                content: result.content,
                meta: Some(result.meta),
                parts: Vec::new(),
            })
            .with_effects(result.effects),
            Err(err) => ask_user_error(call.id, call.name, err),
        }
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &'static str {
        "ask_user"
    }

    fn description(&self) -> &'static str {
        "Ask the user one or more questions and wait for their answers; use only when a decision genuinely belongs to the user"
    }

    fn input_schema(&self) -> Value {
        serde_json::to_value(schemars::schema_for!(AskUserArgs)).unwrap()
    }

    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult {
        let invocation = ToolInvocation {
            invocation_id: format!("inv_{}", call.id),
            call,
        };
        self.handle(invocation, ctx).await.model_result
    }
}

#[derive(Debug, Clone, PartialEq)]
struct AskUserOutcome {
    status: ToolStatus,
    content: String,
    meta: Value,
    effects: Vec<ToolRuntimeEffect>,
}

async fn handle_ask_user(args: &AskUserArgs, ctx: &ToolContext) -> Result<AskUserOutcome, String> {
    if let Some(request_id) = &args.request_id {
        return handle_user_input_resolution(args, ctx, ApprovalId::new(request_id)).await;
    }

    let thread_id = ctx
        .thread_id
        .clone()
        .ok_or_else(|| "ask_user requires a runtime thread_id".to_string())?;
    if ctx
        .policy
        .has_pending_user_input_for_thread(&thread_id)
        .await
    {
        return Err("this thread already has a pending ask_user request; wait for the user to answer or dismiss it".to_string());
    }

    let questions = normalize_questions(
        args.questions
            .clone()
            .ok_or_else(|| "questions is required".to_string())?,
    )?;
    let request = ctx
        .policy
        .create_user_input_request(thread_id.clone(), "ask_user", questions.clone())
        .await;
    let count = questions.len();
    let request_id = request.request_id.clone();

    Ok(AskUserOutcome {
        status: ToolStatus::ReviewRequired,
        content: format!("Waiting for the user to answer {count} question(s)"),
        meta: json!({
            "request_id": request_id.as_str(),
            "question_count": count,
        }),
        effects: vec![ToolRuntimeEffect::UserInputRequested {
            request_id,
            thread_id,
            tool_name: "ask_user".to_string(),
            questions,
        }],
    })
}

async fn handle_user_input_resolution(
    args: &AskUserArgs,
    ctx: &ToolContext,
    request_id: ApprovalId,
) -> Result<AskUserOutcome, String> {
    let decision = args
        .decision
        .as_deref()
        .ok_or_else(|| "decision is required when request_id is provided".to_string())?;
    let pending = ctx.policy.take_pending_user_input(&request_id).await?;
    let dismissed = match decision {
        "answered" => false,
        "dismissed" => true,
        other => {
            return Err(format!(
                "unsupported ask_user decision: {other}; expected answered or dismissed"
            ))
        }
    };

    let content = if dismissed {
        "User dismissed the questions without answering. Proceed using your best judgment."
            .to_string()
    } else {
        format_answers(
            &pending.questions,
            pending.answers.as_deref().unwrap_or(&[]),
        )
    };
    let question_count = pending.questions.len();

    Ok(AskUserOutcome {
        status: ToolStatus::Success,
        content,
        meta: json!({
            "request_id": request_id.as_str(),
            "question_count": question_count,
            "dismissed": dismissed,
        }),
        effects: vec![ToolRuntimeEffect::UserInputResolved {
            request_id,
            dismissed,
        }],
    })
}

fn normalize_questions(questions: Vec<QuestionPrompt>) -> Result<Vec<QuestionPrompt>, String> {
    if questions.is_empty() {
        return Err("ask_user requires at least one question".to_string());
    }
    if questions.len() > MAX_QUESTIONS {
        return Err(format!(
            "ask_user supports at most {MAX_QUESTIONS} questions per request"
        ));
    }

    questions
        .into_iter()
        .enumerate()
        .map(|(index, question)| normalize_question(index, question))
        .collect()
}

fn normalize_question(index: usize, question: QuestionPrompt) -> Result<QuestionPrompt, String> {
    let ordinal = index + 1;
    let text = question.question.trim();
    if text.is_empty() {
        return Err(format!("question {ordinal} must not be empty"));
    }
    if question.options.len() > MAX_OPTIONS_PER_QUESTION {
        return Err(format!(
            "question {ordinal} supports at most {MAX_OPTIONS_PER_QUESTION} options"
        ));
    }
    let header = match question.header {
        Some(header) => {
            let header = header.trim().to_string();
            if header.chars().count() > MAX_HEADER_CHARS {
                return Err(format!(
                    "question {ordinal} header must be at most {MAX_HEADER_CHARS} characters"
                ));
            }
            (!header.is_empty()).then_some(header)
        }
        None => None,
    };
    let options = question
        .options
        .into_iter()
        .enumerate()
        .map(|(option_index, option)| normalize_option(ordinal, option_index, option))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(QuestionPrompt {
        question: text.to_string(),
        header,
        options,
        multi_select: question.multi_select,
    })
}

fn normalize_option(
    question_ordinal: usize,
    option_index: usize,
    option: QuestionOption,
) -> Result<QuestionOption, String> {
    let option_ordinal = option_index + 1;
    let label = option.label.trim();
    if label.is_empty() {
        return Err(format!(
            "question {question_ordinal} option {option_ordinal} label must not be empty"
        ));
    }
    let description = option
        .description
        .map(|description| description.trim().to_string())
        .filter(|description| !description.is_empty());
    Ok(QuestionOption {
        label: label.to_string(),
        description,
    })
}

fn format_answers(questions: &[QuestionPrompt], answers: &[Vec<String>]) -> String {
    let rendered = questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let answer = answers
                .get(index)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| value.trim())
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let answer = if answer.is_empty() {
                "Unanswered".to_string()
            } else {
                answer.join(", ")
            };
            format!("\"{}\" = \"{}\"", question.question, answer)
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("User answered: {rendered}. Continue with these answers in mind.")
}

fn ask_user_error(tool_call_id: String, tool_name: String, content: String) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Error,
        content,
        meta: None,
        parts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::config::AgentConfig;
    use crate::exec_session::ExecSessionManager;
    use crate::policy::PolicyManager;
    use crate::runtime::agent_profile::AgentToolPolicy;
    use crate::types::{ThreadId, TurnId};
    use tempfile::tempdir;

    fn question(text: &str) -> QuestionPrompt {
        QuestionPrompt {
            question: text.to_string(),
            header: Some("Pick".to_string()),
            options: vec![QuestionOption {
                label: "A".to_string(),
                description: None,
            }],
            multi_select: false,
        }
    }

    fn ctx(policy: Arc<PolicyManager>) -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        let workspace_root = dir.path().to_path_buf();
        (
            dir,
            ToolContext {
                config: AgentConfig {
                    workspace_root: workspace_root.clone(),
                    cwd: workspace_root,
                    ..AgentConfig::default()
                },
                thread_id: Some(ThreadId::new("thread_ask_user")),
                turn_id: Some(TurnId::new("turn_ask_user")),
                tool_invocation_id: None,
                exec_sessions: Arc::new(ExecSessionManager::default()),
                exec_output_sink: None,
                policy,
                agent_tool_policy: AgentToolPolicy::all(),
                inbox: None,
                goal_api: None,
                memory_api: None,
            },
        )
    }

    fn call(arguments: Value) -> ToolInvocation {
        ToolInvocation {
            invocation_id: "inv_ask_user".to_string(),
            call: ToolCall {
                id: "call_ask_user".to_string(),
                name: "ask_user".to_string(),
                arguments,
                thought_signature: None,
            },
        }
    }

    #[tokio::test]
    async fn first_call_requests_user_input() {
        let policy = Arc::new(PolicyManager::default());
        let (_dir, ctx) = ctx(policy);
        let outcome = AskUserTool
            .handle(call(json!({"questions": [question("Choose?")]})), &ctx)
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::ReviewRequired);
        assert!(outcome
            .model_result
            .content
            .contains("Waiting for the user"));
        assert!(matches!(
            outcome.effects.as_slice(),
            [ToolRuntimeEffect::UserInputRequested { questions, .. }] if questions[0].question == "Choose?"
        ));
    }

    #[tokio::test]
    async fn validation_rejects_too_many_questions() {
        let policy = Arc::new(PolicyManager::default());
        let (_dir, ctx) = ctx(policy);
        let questions = vec![
            question("1?"),
            question("2?"),
            question("3?"),
            question("4?"),
            question("5?"),
        ];

        let outcome = AskUserTool
            .handle(call(json!({"questions": questions})), &ctx)
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Error);
        assert!(outcome.model_result.content.contains("at most 4"));
    }

    #[tokio::test]
    async fn double_pending_is_rejected() {
        let policy = Arc::new(PolicyManager::default());
        let (_dir, ctx) = ctx(policy);
        let first = AskUserTool
            .handle(call(json!({"questions": [question("First?")]})), &ctx)
            .await;
        assert_eq!(first.model_result.status, ToolStatus::ReviewRequired);

        let second = AskUserTool
            .handle(call(json!({"questions": [question("Second?")]})), &ctx)
            .await;

        assert_eq!(second.model_result.status, ToolStatus::Error);
        assert!(second
            .model_result
            .content
            .contains("already has a pending"));
    }

    #[tokio::test]
    async fn answered_request_formats_answers_and_unanswered() {
        let policy = Arc::new(PolicyManager::default());
        let (_dir, ctx) = ctx(policy.clone());
        let thread_id = ctx.thread_id.clone().unwrap();
        let request = policy
            .create_user_input_request(
                thread_id,
                "ask_user",
                vec![question("Select colors?"), question("Anything else?")],
            )
            .await;
        policy
            .submit_user_input_answers(
                &request.request_id,
                vec![vec!["red".to_string(), "blue".to_string()], vec![]],
            )
            .await
            .unwrap();

        let outcome = AskUserTool
            .handle(
                call(json!({
                    "request_id": request.request_id.as_str(),
                    "decision": "answered"
                })),
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        assert_eq!(
            outcome.model_result.content,
            "User answered: \"Select colors?\" = \"red, blue\"; \"Anything else?\" = \"Unanswered\". Continue with these answers in mind."
        );
        assert!(matches!(
            outcome.effects.as_slice(),
            [ToolRuntimeEffect::UserInputResolved {
                dismissed: false,
                ..
            }]
        ));
    }

    #[tokio::test]
    async fn dismissed_request_is_successful_information() {
        let policy = Arc::new(PolicyManager::default());
        let (_dir, ctx) = ctx(policy.clone());
        let thread_id = ctx.thread_id.clone().unwrap();
        let request = policy
            .create_user_input_request(thread_id, "ask_user", vec![question("Proceed?")])
            .await;

        let outcome = AskUserTool
            .handle(
                call(json!({
                    "request_id": request.request_id.as_str(),
                    "decision": "dismissed"
                })),
                &ctx,
            )
            .await;

        assert_eq!(outcome.model_result.status, ToolStatus::Success);
        assert_eq!(
            outcome.model_result.content,
            "User dismissed the questions without answering. Proceed using your best judgment."
        );
        assert!(matches!(
            outcome.effects.as_slice(),
            [ToolRuntimeEffect::UserInputResolved {
                dismissed: true,
                ..
            }]
        ));
    }
}
