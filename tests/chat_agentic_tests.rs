//! Integration tests for the agentic chat loop (`chat::run_agentic_loop`).
//!
//! The loop is driven by injected `llm_fn` / `search_fn` closures, so these
//! tests exercise the full tool-calling control flow with scripted responses —
//! no network, model, or database involved.

use std::cell::RefCell;

use myrss::chat::{SearchResult, run_agentic_loop, web_search_tool};
use myrss::llm::{ChatMessage, ChatResponse, Role, ToolCall, ToolSpec};

fn initial_messages() -> Vec<ChatMessage> {
    vec![
        ChatMessage::Text {
            role: Role::System,
            content: "system".to_string(),
        },
        ChatMessage::Text {
            role: Role::User,
            content: "question".to_string(),
        },
    ]
}

fn web_search_call(query: &str) -> ChatResponse {
    ChatResponse::ToolCalls {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "web_search".to_string(),
            arguments: serde_json::json!({ "query": query }),
        }],
    }
}

#[test]
fn searches_then_answers_and_feeds_results_back() {
    let llm_calls = RefCell::new(0usize);
    let searched = RefCell::new(Vec::<String>::new());

    let llm = |_msgs: &[ChatMessage], tools: &[ToolSpec]| -> anyhow::Result<ChatResponse> {
        let n = {
            let mut c = llm_calls.borrow_mut();
            let v = *c;
            *c += 1;
            v
        };
        if n == 0 {
            assert!(!tools.is_empty(), "tools must be offered on the first call");
            Ok(web_search_call("rust async 2026"))
        } else {
            Ok(ChatResponse::Message(
                "here is the grounded answer".to_string(),
            ))
        }
    };
    let search = |query: &str| -> anyhow::Result<Vec<SearchResult>> {
        searched.borrow_mut().push(query.to_string());
        Ok(vec![SearchResult {
            title: "Async news".to_string(),
            url: "https://example.com/async".to_string(),
            content: "important async fact".to_string(),
        }])
    };

    let outcome = run_agentic_loop(
        llm,
        search,
        |_| {},
        initial_messages(),
        &[web_search_tool()],
        3,
    )
    .unwrap();

    assert_eq!(outcome.answer, "here is the grounded answer");
    assert_eq!(*searched.borrow(), vec!["rust async 2026"]);
    assert_eq!(*llm_calls.borrow(), 2);

    // The produced turns: assistant tool-call, tool result, final answer.
    assert_eq!(outcome.new_messages.len(), 3);
    assert!(matches!(
        outcome.new_messages[0],
        ChatMessage::AssistantToolCalls { .. }
    ));
    match &outcome.new_messages[1] {
        ChatMessage::ToolResult { content, name, .. } => {
            assert_eq!(name, "web_search");
            // Search results are fed back, wrapped as untrusted data.
            assert!(content.contains("important async fact"));
            assert!(content.contains("<search_results>"));
        }
        other => panic!("expected tool result, got {other:?}"),
    }
    assert!(matches!(
        &outcome.new_messages[2],
        ChatMessage::Text { role: Role::Assistant, content } if content == "here is the grounded answer"
    ));
}

#[test]
fn iteration_cap_withholds_tools_to_force_a_final_answer() {
    // A model that always wants to search must still terminate: once the search
    // budget is spent, tools are withheld and it is forced to answer.
    let max_iterations = 2;
    let searches = RefCell::new(0usize);

    let llm = |_msgs: &[ChatMessage], tools: &[ToolSpec]| -> anyhow::Result<ChatResponse> {
        if tools.is_empty() {
            Ok(ChatResponse::Message("forced final answer".to_string()))
        } else {
            Ok(web_search_call("loop forever"))
        }
    };
    let search = |_query: &str| -> anyhow::Result<Vec<SearchResult>> {
        *searches.borrow_mut() += 1;
        Ok(vec![])
    };

    let outcome = run_agentic_loop(
        llm,
        search,
        |_| {},
        initial_messages(),
        &[web_search_tool()],
        max_iterations,
    )
    .unwrap();

    assert_eq!(outcome.answer, "forced final answer");
    // Exactly `max_iterations` searches run before tools are withheld.
    assert_eq!(*searches.borrow(), max_iterations as usize);
}

#[test]
fn answers_directly_without_searching() {
    let searches = RefCell::new(0usize);
    let llm = |_msgs: &[ChatMessage], _tools: &[ToolSpec]| -> anyhow::Result<ChatResponse> {
        Ok(ChatResponse::Message(
            "answer from the article alone".to_string(),
        ))
    };
    let search = |_query: &str| -> anyhow::Result<Vec<SearchResult>> {
        *searches.borrow_mut() += 1;
        Ok(vec![])
    };

    let outcome = run_agentic_loop(
        llm,
        search,
        |_| {},
        initial_messages(),
        &[web_search_tool()],
        3,
    )
    .unwrap();

    assert_eq!(outcome.answer, "answer from the article alone");
    assert_eq!(
        *searches.borrow(),
        0,
        "no search when the model answers directly"
    );
    assert_eq!(outcome.new_messages.len(), 1);
}
