//! Integration tests for `PandariaAgentExecutor` — end-to-end with
//! `MockProvider`, `SessionActor`, `SquadEngine`, and DAG mode.

use std::collections::HashMap;
use std::sync::Arc;

use ai_provider::test_utils::MockProvider;
use async_trait::async_trait;
use tavern_comp::{
    AgentResolver, HandoffMode, Mission, PandariaAgentExecutor, Role,
    SquadEngine, Team, Visibility,
};
use tavern_core::{AgentConfig, ModelConfig, PlanningConfig, Process};

// ── Helpers ────────────────────────────────────────────────────────────────

/// In-memory `AgentResolver` for tests.
struct HashMapAgentResolver {
    agents: HashMap<String, AgentConfig>,
}

impl HashMapAgentResolver {
    fn new(agents: Vec<AgentConfig>) -> Self {
        Self {
            agents: agents.into_iter().map(|a| (a.id.clone(), a)).collect(),
        }
    }
}

#[async_trait]
impl AgentResolver for HashMapAgentResolver {
    async fn resolve(&self, agent_id: &str) -> Option<AgentConfig> {
        self.agents.get(agent_id).cloned()
    }
}

fn make_agent(id: &str, instructions: &str) -> AgentConfig {
    AgentConfig {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        model: ModelConfig {
            provider: "mock".to_string(),
            name: "mock-v1".to_string(),
            temperature: 0.7,
        },
        instructions: instructions.to_string(),
        skills: vec![],
        constraints: vec![],
        memory: Default::default(),
    }
}

fn make_harness_config(provider: Arc<MockProvider>) -> agent_core::HarnessConfig {
    agent_core::HarnessConfig {
        provider,
        default_model: "mock/mock-v1".to_string(),
        default_system_prompt: "You are a helpful assistant.".to_string(),
        default_context_window: 128_000,
        store: None,
        media_provider: None,
        media_registry: None,
        http_client: reqwest::Client::new(),
        available_models: vec!["mock/mock-v1".to_string()],
        compaction_config: agent_core::CompactionConfig {
            enabled: false,
            ..Default::default()
        },
        agent_space: agent_core::AgentSpace::default(),
        hook_config: agent_core::HookConfig::default(),
        memory_store: None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Smoke test: deploy + run a single-mission squad through PandariaAgentExecutor.
#[tokio::test]
async fn single_mission_smoke() {
    // Provider returns a fixed response to the LLM call
    let provider = Arc::new(MockProvider::text("research complete: AI is transformative"));

    let agent = make_agent("researcher", "You are a researcher.");
    let resolver = Arc::new(HashMapAgentResolver::new(vec![agent]));

    let harness_config = make_harness_config(provider);
    let executor = Arc::new(PandariaAgentExecutor::new(
        "test-tenant",
        "test-team",
        harness_config,
        resolver,
    ));

    let team = Team {
        id: "research_team".into(),
        name: "Research Team".into(),
        description: None,
        roles: vec![Role {
            id: "researcher".into(),
            name: "Researcher".into(),
            agent_id: "researcher".into(),
            visibility: Visibility::default(),
            ..Default::default()
        }],
        missions: vec![Mission {
            id: "research_mission".into(),
            role: "researcher".into(),
            task: "research AI trends".into(),
            output_key: Some("findings".into()),
            handoff_mode: HandoffMode::Inherit,
            ..Default::default()
        }],
        default_process: Process::Sequential,
        planning: None,
        webhook: None,
    };

    let engine = SquadEngine::new();
    let mut squad = engine
        .deploy(&team, executor, serde_json::json!({}))
        .await
        .expect("deploy squad");

    let result = engine
        .run(&team, &mut squad)
        .await
        .expect("run squad");

    assert_eq!(result.status, tavern_comp::SquadStatus::Completed);
    assert_eq!(
        result.outputs.get("findings").unwrap(),
        "research complete: AI is transformative"
    );
    // team_id in SquadResult comes from Team.id
    assert_eq!(result.team_id, "research_team");
}

/// Two-mission sequential pipeline: researcher → writer with context passing.
#[tokio::test]
async fn two_mission_sequential_pipeline() {
    // First call: researcher output, second call: writer output
    let provider = Arc::new(MockProvider::sequence(vec![
        ai_provider::test_utils::MockResponse::Text("raw research notes".into()),
        ai_provider::test_utils::MockResponse::Text("polished article".into()),
    ]));

    let agents = vec![
        make_agent("researcher", "You are a researcher."),
        make_agent("writer", "You are a writer."),
    ];
    let resolver = Arc::new(HashMapAgentResolver::new(agents));

    let harness_config = make_harness_config(provider);
    let executor = Arc::new(PandariaAgentExecutor::new(
        "test-tenant",
        "test-team",
        harness_config,
        resolver,
    ));

    let team = Team {
        id: "content_team".into(),
        name: "Content Team".into(),
        description: None,
        roles: vec![
            Role {
                id: "researcher".into(),
                name: "Researcher".into(),
                agent_id: "researcher".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
            Role {
                id: "writer".into(),
                name: "Writer".into(),
                agent_id: "writer".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
        ],
        missions: vec![
            Mission {
                id: "research".into(),
                role: "researcher".into(),
                task: "research {{topic}}".into(),
                output_key: Some("notes".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "write".into(),
                role: "writer".into(),
                task: "write from {{notes}}".into(),
                depends_on: vec!["research".into()],
                output_key: Some("article".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
        ],
        default_process: Process::Sequential,
        planning: None,
        webhook: None,
    };

    let engine = SquadEngine::new();
    let mut squad = engine
        .deploy(&team, executor, serde_json::json!({"topic": "AI"}))
        .await
        .expect("deploy squad");

    let result = engine
        .run(&team, &mut squad)
        .await
        .expect("run squad");

    assert_eq!(result.status, tavern_comp::SquadStatus::Completed);
    assert_eq!(result.outputs.get("notes").unwrap(), "raw research notes");
    assert_eq!(result.outputs.get("article").unwrap(), "polished article");
}

/// DAG execution with parallel branches.
#[tokio::test]
async fn dag_parallel_branches() {
    // Sequence: root, then left+right in parallel, then merge
    let provider = Arc::new(MockProvider::sequence(vec![
        ai_provider::test_utils::MockResponse::Text("root result".into()),
        ai_provider::test_utils::MockResponse::Text("left branch".into()),
        ai_provider::test_utils::MockResponse::Text("right branch".into()),
        ai_provider::test_utils::MockResponse::Text("merged result".into()),
    ]));

    let agents = vec![
        make_agent("researcher", "You are a researcher."),
        make_agent("analyst", "You are an analyst."),
        make_agent("editor", "You are an editor."),
    ];
    let resolver = Arc::new(HashMapAgentResolver::new(agents));

    let harness_config = make_harness_config(provider);
    let executor = Arc::new(PandariaAgentExecutor::new(
        "test-tenant",
        "test-team",
        harness_config,
        resolver,
    ));

    let team = Team {
        id: "dag_team".into(),
        name: "DAG Team".into(),
        description: None,
        roles: vec![
            Role {
                id: "researcher".into(),
                name: "Researcher".into(),
                agent_id: "researcher".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
            Role {
                id: "analyst".into(),
                name: "Analyst".into(),
                agent_id: "analyst".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
            Role {
                id: "editor".into(),
                name: "Editor".into(),
                agent_id: "editor".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
        ],
        missions: vec![
            Mission {
                id: "root".into(),
                role: "researcher".into(),
                task: "root task".into(),
                output_key: Some("root_out".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "left".into(),
                role: "analyst".into(),
                task: "left from {{root_out}}".into(),
                depends_on: vec!["root".into()],
                output_key: Some("left_out".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "right".into(),
                role: "analyst".into(),
                task: "right from {{root_out}}".into(),
                depends_on: vec!["root".into()],
                output_key: Some("right_out".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "merge".into(),
                role: "editor".into(),
                task: "merge {{left_out}} {{right_out}}".into(),
                depends_on: vec!["left".into(), "right".into()],
                output_key: Some("final".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
        ],
        default_process: Process::Sequential,
        planning: None,
        webhook: None,
    };

    let engine = SquadEngine::new().with_max_concurrency(2);
    let mut squad = engine
        .deploy(&team, executor, serde_json::json!({}))
        .await
        .expect("deploy squad");

    let result = engine
        .run(&team, &mut squad)
        .await
        .expect("run squad");

    assert_eq!(result.status, tavern_comp::SquadStatus::Completed);
    assert_eq!(result.outputs.get("root_out").unwrap(), "root result");
    // Parallel branches: order is non-deterministic, so just check both exist
    assert!(result.outputs.get("left_out").is_some(), "left_out should be present");
    assert!(result.outputs.get("right_out").is_some(), "right_out should be present");
    assert_eq!(result.outputs.get("final").unwrap(), "merged result");
}

/// Session reuse: second mission with same role+model reuses the SessionActor.
#[tokio::test]
async fn session_reuse_across_missions() {
    let provider = Arc::new(MockProvider::sequence(vec![
        ai_provider::test_utils::MockResponse::Text("first result".into()),
        ai_provider::test_utils::MockResponse::Text("second result".into()),
    ]));

    let agent = make_agent("worker", "You are a worker.");
    let resolver = Arc::new(HashMapAgentResolver::new(vec![agent]));

    let harness_config = make_harness_config(provider);
    let executor = Arc::new(PandariaAgentExecutor::new(
        "test-tenant",
        "test-team",
        harness_config,
        resolver,
    ));

    let team = Team {
        id: "reuse_team".into(),
        name: "Reuse Team".into(),
        description: None,
        roles: vec![Role {
            id: "worker".into(),
            name: "Worker".into(),
            agent_id: "worker".into(),
            visibility: Visibility::default(),
            ..Default::default()
        }],
        missions: vec![
            Mission {
                id: "task1".into(),
                role: "worker".into(),
                task: "task 1".into(),
                output_key: Some("out1".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "task2".into(),
                role: "worker".into(),
                task: "task 2".into(),
                depends_on: vec!["task1".into()],
                output_key: Some("out2".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
        ],
        default_process: Process::Sequential,
        planning: None,
        webhook: None,
    };

    let engine = SquadEngine::new();
    let mut squad = engine
        .deploy(&team, executor.clone(), serde_json::json!({}))
        .await
        .expect("deploy squad");

    let result = engine
        .run(&team, &mut squad)
        .await
        .expect("run squad");

    assert_eq!(result.status, tavern_comp::SquadStatus::Completed);
    assert_eq!(result.outputs.get("out1").unwrap(), "first result");
    assert_eq!(result.outputs.get("out2").unwrap(), "second result");

    // Verify session was cached (same role, same model = one cache entry)
    let session_count = executor.session_count();
    assert_eq!(session_count, 1, "expected one cached session for worker:mock/mock-v1");
}

/// Model override creates a separate session.
#[tokio::test]
async fn model_override_creates_separate_session() {
    let provider = Arc::new(MockProvider::sequence(vec![
        ai_provider::test_utils::MockResponse::Text("result from gpt".into()),
        ai_provider::test_utils::MockResponse::Text("result from claude".into()),
    ]));

    let agent_default = AgentConfig {
        id: "worker_default".into(),
        name: "worker_default".into(),
        description: None,
        model: ModelConfig {
            provider: "mock".into(),
            name: "mock-v1".into(),
            temperature: 0.7,
        },
        instructions: "You are a worker.".into(),
        skills: vec![],
        constraints: vec![],
        memory: Default::default(),
    };

    let agent_claude = AgentConfig {
        id: "worker_claude".into(),
        name: "worker_claude".into(),
        description: None,
        model: ModelConfig {
            provider: "anthropic".into(),
            name: "claude".into(),
            temperature: 0.7,
        },
        instructions: "You are a worker.".into(),
        skills: vec![],
        constraints: vec![],
        memory: Default::default(),
    };

    let resolver = Arc::new(HashMapAgentResolver::new(vec![
        agent_default,
        agent_claude,
    ]));

    let harness_config = make_harness_config(provider);
    let executor = Arc::new(PandariaAgentExecutor::new(
        "test-tenant",
        "test-team",
        harness_config,
        resolver,
    ));

    let team = Team {
        id: "model_team".into(),
        name: "Model Team".into(),
        description: None,
        roles: vec![
            Role {
                id: "worker_default".into(),
                name: "Worker Default".into(),
                agent_id: "worker_default".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
            Role {
                id: "worker_claude".into(),
                name: "Worker Claude".into(),
                agent_id: "worker_claude".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
        ],
        missions: vec![
            Mission {
                id: "task_default".into(),
                role: "worker_default".into(),
                task: "default".into(),
                output_key: Some("out1".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "task_claude".into(),
                role: "worker_claude".into(),
                task: "claude".into(),
                depends_on: vec!["task_default".into()],
                output_key: Some("out2".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
        ],
        default_process: Process::Sequential,
        planning: None,
        webhook: None,
    };

    let engine = SquadEngine::new();
    let mut squad = engine
        .deploy(&team, executor.clone(), serde_json::json!({}))
        .await
        .expect("deploy squad");

    let result = engine
        .run(&team, &mut squad)
        .await
        .expect("run squad");

    assert_eq!(result.status, tavern_comp::SquadStatus::Completed);

    // Two different agents with different models = two cache entries
    let session_count = executor.session_count();
    assert_eq!(
        session_count, 2,
        "expected 2 cached sessions: mock/mock-v1 and anthropic/claude"
    );
}

/// Hierarchical Manager-Worker mode: manager delegates missions to workers,
/// then terminates when all done.
#[tokio::test]
async fn hierarchical_manager_delegates() {
    use tavern_core::ManagerConfig;

    // Sequence: manager(→researcher) → researcher → manager(→writer) → writer → manager(terminate)
    let provider = Arc::new(MockProvider::sequence(vec![
        // Manager call 1: delegate to researcher
        ai_provider::test_utils::MockResponse::Text(
            r#"{"summary": "start with research", "next_role": "researcher"}"#.into(),
        ),
        // Researcher call
        ai_provider::test_utils::MockResponse::Text("research notes about AI".into()),
        // Manager call 2: delegate to writer
        ai_provider::test_utils::MockResponse::Text(
            r#"{"summary": "now write", "next_role": "writer"}"#.into(),
        ),
        // Writer call
        ai_provider::test_utils::MockResponse::Text("final article text".into()),
        // Manager call 3: terminate
        ai_provider::test_utils::MockResponse::Text(
            r#"{"summary": "done", "terminate": true}"#.into(),
        ),
    ]));

    let agents = vec![
        make_agent("manager", "You are a project manager."),
        make_agent("researcher", "You are a researcher."),
        make_agent("writer", "You are a writer."),
    ];
    let resolver = Arc::new(HashMapAgentResolver::new(agents));

    let harness_config = make_harness_config(provider);
    let executor = Arc::new(PandariaAgentExecutor::new(
        "test-tenant",
        "test-team",
        harness_config,
        resolver,
    ));

    let team = Team {
        id: "content_team".into(),
        name: "Content Team".into(),
        description: None,
        roles: vec![
            Role {
                id: "manager".into(),
                name: "Manager".into(),
                agent_id: "manager".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
            Role {
                id: "researcher".into(),
                name: "Researcher".into(),
                agent_id: "researcher".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
            Role {
                id: "writer".into(),
                name: "Writer".into(),
                agent_id: "writer".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
        ],
        missions: vec![
            Mission {
                id: "research".into(),
                role: "researcher".into(),
                task: "research {{topic}}".into(),
                output_key: Some("notes".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "write".into(),
                role: "writer".into(),
                task: "write from {{notes}}".into(),
                output_key: Some("article".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
        ],
        default_process: Process::Hierarchical(ManagerConfig {
            agent_id: "manager".into(),
            instructions: None,
        }),
        planning: None,
        webhook: None,
    };

    let engine = SquadEngine::new();
    let mut squad = engine
        .deploy(&team, executor, serde_json::json!({ "topic": "AI" }))
        .await
        .expect("deploy squad");

    let result = engine
        .run(&team, &mut squad)
        .await
        .expect("run squad");

    assert_eq!(result.status, tavern_comp::SquadStatus::Completed);
    assert_eq!(result.outputs.get("notes").unwrap(), "research notes about AI");
    assert_eq!(result.outputs.get("article").unwrap(), "final article text");
}

/// Planning phase: planner agent analyzes missions and injects plan context.
#[tokio::test]
async fn planning_phase_injects_context() {
    // Sequence: planner (returns plan JSON) → researcher → writer
    let provider = Arc::new(MockProvider::sequence(vec![
        // Planner call: returns a plan with overall strategy and per-mission reasoning
        ai_provider::test_utils::MockResponse::Text(
            serde_json::json!({
                "overall_strategy": "Research the topic first, then write the article based on findings.",
                "steps": [
                    {
                        "task_id": "research",
                        "agent_id": "researcher",
                        "reasoning": "Need to gather facts before writing.",
                        "expected_output": "A summary of key findings about AI",
                        "dependencies": []
                    },
                    {
                        "task_id": "write",
                        "agent_id": "writer",
                        "reasoning": "Use research findings to produce the article.",
                        "expected_output": "A well-written article on AI",
                        "dependencies": []
                    }
                ]
            }).to_string().into(),
        ),
        // Researcher call
        ai_provider::test_utils::MockResponse::Text("research notes about AI".into()),
        // Writer call
        ai_provider::test_utils::MockResponse::Text("final article text".into()),
    ]));

    let agents = vec![
        make_agent("planner", "You are a planning agent."),
        make_agent("researcher", "You are a researcher."),
        make_agent("writer", "You are a writer."),
    ];
    let resolver = Arc::new(HashMapAgentResolver::new(agents));

    let harness_config = make_harness_config(provider);
    let executor = Arc::new(PandariaAgentExecutor::new(
        "test-tenant",
        "test-team",
        harness_config,
        resolver,
    ));

    let team = Team {
        id: "planning_team".into(),
        name: "Planning Team".into(),
        description: None,
        roles: vec![
            Role {
                id: "planner".into(),
                name: "Planner".into(),
                agent_id: "planner".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
            Role {
                id: "researcher".into(),
                name: "Researcher".into(),
                agent_id: "researcher".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
            Role {
                id: "writer".into(),
                name: "Writer".into(),
                agent_id: "writer".into(),
                visibility: Visibility::default(),
                ..Default::default()
            },
        ],
        missions: vec![
            Mission {
                id: "research".into(),
                role: "researcher".into(),
                task: "research {{topic}}".into(),
                output_key: Some("notes".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
            Mission {
                id: "write".into(),
                role: "writer".into(),
                task: "write from {{notes}}".into(),
                depends_on: vec!["research".into()],
                output_key: Some("article".into()),
                handoff_mode: HandoffMode::Inherit,
                ..Default::default()
            },
        ],
        default_process: Process::Sequential,
        planning: Some(PlanningConfig {
            enabled: true,
            planning_agent: Some("planner".into()),
        }),
        webhook: None,
    };

    let engine = SquadEngine::new();
    let mut squad = engine
        .deploy(&team, executor, serde_json::json!({ "topic": "AI" }))
        .await
        .expect("deploy squad");

    let result = engine
        .run(&team, &mut squad)
        .await
        .expect("run squad");

    assert_eq!(result.status, tavern_comp::SquadStatus::Completed);
    assert_eq!(result.outputs.get("notes").unwrap(), "research notes about AI");
    assert_eq!(result.outputs.get("article").unwrap(), "final article text");
}
