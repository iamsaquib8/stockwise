//! `ask` against a mock AI server for each provider: the requests it sends,
//! how it runs the tools and feeds results back, and its limits. No real AI
//! API is ever called.

use super::*;
use crate::market::Market;
use crate::testutil::MockServer;
use crate::views::fixtures::Fixture;

fn ctx() -> ToolContext<Fixture> {
    ToolContext::new(Fixture::default(), Market::In)
}

fn client(provider: AiProvider, url: &str) -> AiClient {
    let key = provider.is_cloud().then(|| "sk-test-123".to_string());
    AiClient::with_provider(provider, key, Some(url.to_string()))
}

async fn run(
    provider: AiProvider,
    replies: Vec<Value>,
    opts: AskOptions,
) -> (Result<Transcript>, Vec<crate::testutil::Recorded>) {
    let server =
        MockServer::start(replies.into_iter().map(|r| (200, r.to_string())).collect()).await;
    let ai = client(provider, &server.url);
    let t = ask(&ai, &ctx(), "How is TCS doing?", &opts).await;
    (t, server.requests())
}

#[tokio::test]
async fn ollama_runs_tool_calls_and_returns_the_answer() {
    let (t, reqs) = run(
        AiProvider::Ollama,
        vec![
            json!({
                "message": {"role": "assistant", "content": "", "tool_calls": [
                    {"function": {"name": "quote", "arguments": {"symbols": ["TCS"]}}}
                ]},
                "prompt_eval_count": 900, "eval_count": 20, "done_reason": "stop",
            }),
            json!({
                "message": {"role": "assistant", "content": "<think>numbers</think>TCS trades at ₹4,100, up 1.5% today."},
                "prompt_eval_count": 1200, "eval_count": 30, "done_reason": "stop",
            }),
        ],
        AskOptions::default(),
    )
    .await;
    let t = t.unwrap();
    assert_eq!(t.answer, "TCS trades at ₹4,100, up 1.5% today.");
    assert_eq!(t.steps.len(), 1);
    assert_eq!(t.steps[0].tool, "quote");
    assert!(t.steps[0].ok);
    assert_eq!(t.usage.requests, 2);
    assert_eq!(t.usage.input_tokens, 2100);
    assert!(t.stopped_early.is_none());

    assert_eq!(reqs[0].target, "/api/chat");
    let first = reqs[0].json();
    assert_eq!(first["stream"], false);
    assert_eq!(first["options"]["num_ctx"], OLLAMA_NUM_CTX);
    assert_eq!(first["messages"][0]["role"], "system");
    assert_eq!(first["messages"][1]["content"], "How is TCS doing?");
    let tools = first["tools"].as_array().unwrap();
    assert_eq!(tools.len(), crate::tools::TOOLS.len());
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "quote");
    assert_eq!(tools[0]["function"]["parameters"]["type"], "object");

    let second = reqs[1].json();
    let msgs = second["messages"].as_array().unwrap();
    assert_eq!(msgs[2]["role"], "assistant");
    assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "quote");
    assert_eq!(msgs[3]["role"], "tool");
    assert_eq!(msgs[3]["tool_name"], "quote");
    let result: Value = serde_json::from_str(msgs[3]["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["data"]["quotes"][0]["symbol"], "TCS.NS");
    assert_eq!(result["note"], NOTE);
}

#[tokio::test]
async fn ollama_models_without_tools_get_a_clear_error() {
    let server = MockServer::start(vec![(
        400,
        r#"{"error":"registry.ollama.ai/library/gemma:2b does not support tools"}"#.into(),
    )])
    .await;
    let ai = client(AiProvider::Ollama, &server.url);
    let err = ask(&ai, &ctx(), "hi", &AskOptions::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not support tool calling"), "{err}");
    assert!(err.contains("ollama pull"), "{err}");

    let server = MockServer::start(vec![(
        404,
        r#"{"error":"model 'qwen9:1b' not found"}"#.into(),
    )])
    .await;
    let ai = client(AiProvider::Ollama, &server.url);
    let err = ask(&ai, &ctx(), "hi", &AskOptions::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("is not installed"), "{err}");
}

#[tokio::test]
async fn openai_sends_function_outputs_with_the_reasoning_back() {
    let reasoning =
        json!({"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "gAAAA"});
    let call = json!({
        "type": "function_call", "id": "fc_1", "call_id": "call_1",
        "name": "goal_sip", "arguments": "{\"target\": 5000000, \"years\": 10}",
    });
    let (t, reqs) = run(
        AiProvider::OpenAi,
        vec![
            json!({"status": "completed", "output": [reasoning, call], "usage": {"input_tokens": 500, "output_tokens": 40}}),
            json!({"status": "completed", "output": [
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "About ₹21,500 a month."}]}
            ], "usage": {"input_tokens": 900, "output_tokens": 20}}),
        ],
        AskOptions::default(),
    )
    .await;
    let t = t.unwrap();
    assert_eq!(t.answer, "About ₹21,500 a month.");
    assert_eq!(t.steps[0].arguments["years"], 10);

    assert_eq!(reqs[0].target, "/v1/responses");
    assert_eq!(reqs[0].header("authorization"), Some("Bearer sk-test-123"));
    let first = reqs[0].json();
    assert_eq!(first["store"], false);
    assert_eq!(first["include"], json!(["reasoning.encrypted_content"]));
    assert!(
        first["instructions"]
            .as_str()
            .unwrap()
            .contains("StockWise")
    );
    assert_eq!(first["tools"][0]["type"], "function");
    assert_eq!(first["tools"][0]["strict"], false);
    assert!(first.get("tool_choice").is_none());

    let input = reqs[1].json()["input"].as_array().unwrap().clone();
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[1], reasoning);
    assert_eq!(input[2], call);
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call_1");
    let out: Value = serde_json::from_str(input[3]["output"].as_str().unwrap()).unwrap();
    assert_eq!(out["data"]["goal"], "sip");
}

#[tokio::test]
async fn anthropic_returns_every_result_in_one_message() {
    let content = json!([
        {"type": "thinking", "thinking": "", "signature": "sig=="},
        {"type": "text", "text": "Checking."},
        {"type": "tool_use", "id": "toolu_1", "name": "watchlist", "input": {}},
        {"type": "tool_use", "id": "toolu_2", "name": "goal_sip", "input": {"target": 100000}},
    ]);
    let (t, reqs) = run(
        AiProvider::Anthropic,
        vec![
            json!({"type": "message", "stop_reason": "tool_use", "content": content,
                   "usage": {"input_tokens": 700, "output_tokens": 60}}),
            json!({"type": "message", "stop_reason": "end_turn",
                   "content": [{"type": "text", "text": "Your watchlist is up today."}],
                   "usage": {"input_tokens": 1500, "output_tokens": 25}}),
        ],
        AskOptions::default(),
    )
    .await;
    let t = t.unwrap();
    assert_eq!(t.answer, "Your watchlist is up today.");
    assert_eq!(t.steps.len(), 2);
    assert!(t.steps[0].ok);
    // The model forgot `years`: it gets the error back, marked as one.
    assert!(!t.steps[1].ok);

    assert_eq!(reqs[0].target, "/v1/messages");
    assert_eq!(reqs[0].header("x-api-key"), Some("sk-test-123"));
    assert_eq!(reqs[0].header("anthropic-version"), Some("2023-06-01"));
    let first = reqs[0].json();
    assert!(first["system"].as_str().unwrap().contains("StockWise"));
    assert_eq!(first["tools"][0]["name"], "quote");
    assert_eq!(first["tools"][0]["input_schema"]["type"], "object");
    assert!(first["max_tokens"].as_u64().unwrap() > 0);
    if client(AiProvider::Anthropic, "http://x").anthropic_fallbacks() {
        assert_eq!(first["fallbacks"], "default");
    }

    let msgs = reqs[1].json()["messages"].as_array().unwrap().clone();
    // The assistant turn goes back exactly as received, thinking included.
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(msgs[1]["content"], content);
    assert_eq!(msgs.len(), 3);
    let results = msgs[2]["content"].as_array().unwrap();
    assert_eq!(msgs[2]["role"], "user");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["type"], "tool_result");
    assert_eq!(results[0]["tool_use_id"], "toolu_1");
    assert!(results[0].get("is_error").is_none());
    assert_eq!(results[1]["tool_use_id"], "toolu_2");
    assert_eq!(results[1]["is_error"], true);
    assert!(results[1]["content"].as_str().unwrap().contains("years"));
}

#[tokio::test]
async fn anthropic_refusals_and_cut_off_tool_calls_are_errors() {
    let (t, _) = run(
        AiProvider::Anthropic,
        vec![
            json!({"stop_reason": "refusal", "stop_details": {"category": "cyber"}, "content": []}),
        ],
        AskOptions::default(),
    )
    .await;
    let e = t.unwrap_err().to_string();
    assert!(e.contains("declined") && e.contains("cyber"), "{e}");

    let (t, reqs) = run(
        AiProvider::Anthropic,
        vec![json!({"stop_reason": "max_tokens", "content": [
            {"type": "tool_use", "id": "toolu_1", "name": "quote", "input": {"symbols": ["TC"]}}
        ], "usage": {"output_tokens": 16000}})],
        AskOptions::default(),
    )
    .await;
    assert!(t.unwrap_err().to_string().contains("cut off"));
    // The cut-off call never ran: one request, nothing after it.
    assert_eq!(reqs.len(), 1);
}

#[tokio::test]
async fn gemini_answers_function_calls_with_function_responses() {
    let model_turn = json!({"role": "model", "parts": [
        {"functionCall": {"name": "markets", "args": {}}, "thoughtSignature": "c2ln"}
    ]});
    let (t, reqs) = run(
        AiProvider::Gemini,
        vec![
            json!({"candidates": [{"content": model_turn, "finishReason": "STOP"}],
                   "usageMetadata": {"promptTokenCount": 800, "candidatesTokenCount": 10}}),
            json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "NIFTY is up 0.5%."}]}, "finishReason": "STOP"}]}),
        ],
        AskOptions::default(),
    )
    .await;
    let t = t.unwrap();
    assert_eq!(t.answer, "NIFTY is up 0.5%.");

    let model = client(AiProvider::Gemini, "http://x").model().to_string();
    assert_eq!(
        reqs[0].target,
        format!("/v1beta/models/{model}:generateContent")
    );
    assert_eq!(reqs[0].header("x-goog-api-key"), Some("sk-test-123"));
    assert!(!reqs[0].target.contains("sk-test"));
    let first = reqs[0].json();
    let decls = first["tools"][0]["functionDeclarations"]
        .as_array()
        .unwrap();
    assert_eq!(decls[0]["name"], "quote");
    assert_eq!(decls[0]["parametersJsonSchema"]["type"], "object");
    assert!(first["systemInstruction"]["parts"][0]["text"].is_string());

    let contents = reqs[1].json()["contents"].as_array().unwrap().clone();
    // The model's turn goes back verbatim, thought signature included.
    assert_eq!(contents[1], model_turn);
    let response = &contents[2]["parts"][0]["functionResponse"];
    assert_eq!(contents[2]["role"], "user");
    assert_eq!(response["name"], "markets");
    assert_eq!(
        response["response"]["data"]["indices"][0]["symbol"],
        "^NSEI"
    );
}

#[tokio::test]
async fn the_last_request_has_no_tools() {
    let call = json!({
        "message": {"role": "assistant", "content": "", "tool_calls": [
            {"function": {"name": "watchlist", "arguments": {}}}
        ]},
    });
    let answer = json!({"message": {"role": "assistant", "content": "Done."}});
    let (t, reqs) = run(
        AiProvider::Ollama,
        vec![call, answer],
        AskOptions {
            max_steps: 2,
            ..Default::default()
        },
    )
    .await;
    let t = t.unwrap();
    assert_eq!(t.answer, "Done.");
    assert!(reqs[0].json().get("tools").is_some());
    assert!(reqs[1].json().get("tools").is_none());
    assert_eq!(t.stopped_early.as_deref(), Some("reached 2 model requests"));

    // A model that still wants tools on its last turn and says nothing.
    let stubborn = json!({
        "message": {"role": "assistant", "content": "", "tool_calls": [
            {"function": {"name": "watchlist", "arguments": {}}}
        ]},
    });
    let (t, reqs) = run(
        AiProvider::Ollama,
        vec![stubborn],
        AskOptions {
            max_steps: 3,
            ..Default::default()
        },
    )
    .await;
    let e = t.unwrap_err().to_string();
    assert!(e.contains("returned no answer"), "{e}");
    assert_eq!(reqs.len(), 3);
}

#[tokio::test]
async fn tool_calls_and_tokens_are_capped() {
    let two_calls = json!({
        "message": {"role": "assistant", "content": "", "tool_calls": [
            {"function": {"name": "watchlist", "arguments": {}}},
            {"function": {"name": "alerts_list", "arguments": {}}}
        ]},
        "prompt_eval_count": 10, "eval_count": 10,
    });
    let answer = json!({"message": {"role": "assistant", "content": "Here is what I found."}});
    let (t, reqs) = run(
        AiProvider::Ollama,
        vec![two_calls.clone(), answer.clone()],
        AskOptions {
            max_tool_calls: 1,
            ..Default::default()
        },
    )
    .await;
    let t = t.unwrap();
    assert_eq!(t.usage.tool_calls, 1);
    assert!(t.steps[0].ok);
    assert!(t.steps[1].error.as_deref().unwrap().contains("limit"));
    // Out of tool calls: the next request must answer.
    assert!(reqs[1].json().get("tools").is_none());

    let big = json!({
        "message": {"role": "assistant", "content": "", "tool_calls": [
            {"function": {"name": "watchlist", "arguments": {}}}
        ]},
        "prompt_eval_count": 5000, "eval_count": 100,
    });
    let (t, reqs) = run(
        AiProvider::Ollama,
        vec![big, answer],
        AskOptions {
            token_budget: 2000,
            ..Default::default()
        },
    )
    .await;
    let t = t.unwrap();
    assert!(t.stopped_early.unwrap().contains("tokens"));
    assert!(reqs[1].json().get("tools").is_none());
}

#[tokio::test]
async fn unknown_tools_and_bad_arguments_go_back_to_the_model() {
    let (t, reqs) = run(
        AiProvider::Ollama,
        vec![
            json!({"message": {"role": "assistant", "content": "", "tool_calls": [
                {"function": {"name": "place_order", "arguments": {"symbol": "TCS"}}}
            ]}}),
            json!({"message": {"role": "assistant", "content": "I can't place orders."}}),
        ],
        AskOptions::default(),
    )
    .await;
    let t = t.unwrap();
    assert!(!t.steps[0].ok);
    let msgs = reqs[1].json()["messages"].as_array().unwrap().clone();
    assert!(
        msgs[3]["content"]
            .as_str()
            .unwrap()
            .contains("Unknown tool 'place_order'")
    );
}

#[test]
fn transcript_json_shape() {
    let t = Transcript {
        question: "q".into(),
        provider: "Ollama",
        model: "m".into(),
        answer: "a".into(),
        steps: vec![Step {
            request: 1,
            tool: "quote".into(),
            arguments: json!({"symbols": ["TCS"]}),
            ok: true,
            error: None,
            result_bytes: 10,
        }],
        usage: Usage::default(),
        stopped_early: None,
        note: NOTE,
    };
    let v = serde_json::to_value(&t).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "question",
            "provider",
            "model",
            "answer",
            "steps",
            "usage",
            "stopped_early",
            "note"
        ]
    );
    let usage: Vec<&str> = v["usage"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        usage,
        ["requests", "tool_calls", "input_tokens", "output_tokens"]
    );
}

#[test]
fn system_prompt_names_the_market_and_the_rules() {
    let p = system_prompt(Market::Us);
    assert!(p.starts_with(crate::ai::prompts::RULES));
    assert!(p.contains("`us`"));
    assert!(p.contains("cannot place orders"));
}
