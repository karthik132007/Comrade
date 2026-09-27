/** Live API checks. Ignored by default: `cargo test -p comrade-core -- --ignored`. */
use comrade_core::config::load_config;
use comrade_core::config::find_project_root;
use comrade_core::llm::{ChatMessage, ChatOptions, Embedder, LlmProvider, OpenRouterEmbedder, OpenRouterProvider};
use comrade_core::llm::factory::create_configured;
use comrade_core::speech::{OpenRouterStt, OpenRouterTts, SpeechToText, TextToSpeech};

fn test_config() -> comrade_core::config::ComradeConfig {
    load_config(&find_project_root())
}

#[tokio::test]
#[ignore]
async fn live_chat_round_trip() {
    let cfg = test_config();
    let llm = create_configured(&cfg);
    let res = llm
        .chat(&[ChatMessage::user("Reply with exactly: COMRADE-OK")], &ChatOptions {
            // deepseek-flash is a reasoning model: budget must cover reasoning + answer.
            max_tokens: 256,
            temperature: 0.0,
            ..ChatOptions::default()
        })
        .await
        .expect("chat call failed");
    assert!(res.content.contains("COMRADE-OK"), "unexpected reply: {}", res.content);
}

#[tokio::test]
#[ignore]
async fn live_tts_stt_round_trip() {    let cfg = test_config();
    let tts = OpenRouterTts::new(
        cfg.openrouter_key.clone(),
        cfg.tts_model.clone(),
        cfg.tts_voice.clone(),
    );
    let stt = OpenRouterStt::new(cfg.openrouter_key.clone(), cfg.stt_model.clone());
    let speech = tts
        .synthesize("Hello Comrade, voice check complete.", None)
        .await
        .expect("tts call failed");
    assert!(speech.mime_type.starts_with("audio/"));
    assert!(!speech.audio.is_empty());
    let text = stt.transcribe(&speech.audio, None, "mp3").await.expect("stt call failed");
    assert!(text.to_lowercase().contains("comrade"), "unexpected transcript: {text}");
}

#[tokio::test]
#[ignore]
async fn live_embedding_round_trip() {
    let cfg = test_config();
    let emb = OpenRouterEmbedder::new(cfg.openrouter_key.clone(), cfg.embedding_model.clone());
    let vecs = emb
        .embed(&["The capital of France is Paris.".to_string(), "Comrade is a desktop agent.".to_string()])
        .await
        .expect("embed call failed");
    assert_eq!(vecs.len(), 2);
    assert_eq!(vecs[0].len(), cfg.embedding_dim, "unexpected embedding dim");
    assert!(vecs.iter().all(|v| v.iter().any(|&x| x != 0.0)));
}

/// Regression: DeepSeek rejects dots in function names (400). The provider
/// must encode them on the wire and decode tool calls back.
#[tokio::test]
#[ignore]
async fn live_deepseek_tool_call_round_trip() {
    use comrade_core::llm::ToolDefinition;
    let cfg = test_config();
    let llm = create_configured(&cfg);
    let res = llm
        .chat(
            &[ChatMessage::user("Echo the word hi with the terminal tool.")],
            &ChatOptions {
                max_tokens: 512,
                temperature: 0.0,
                tools: vec![ToolDefinition {
                    name: "terminal.execute".to_string(),
                    description: "Run a shell command.".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": { "command": { "type": "string" } },
                        "required": ["command"],
                    }),
                }],
                ..ChatOptions::default()
            },
        )
        .await
        .expect("tool chat call failed (400 means wire names are invalid)");
    for call in &res.tool_calls {
        assert!(!call.name.contains('_'), "tool name not decoded: {}", call.name);
    }
}
