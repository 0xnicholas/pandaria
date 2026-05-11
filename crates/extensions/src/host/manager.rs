use std::collections::HashSet;
use std::sync::Arc;

use agent_core::types::AgentToolRef;
use llm_client::ToolDef;

use super::event_bus::EventBus;
use super::extension::Extension;
use super::extension_actor::{ExtensionActor, ExtensionHandle};
use super::extension_tool::ExtensionTool;
use super::hook_router::HookRouter;

/// Manages the lifecycle of extensions for a single session.
///
/// - Collects tool definitions from all extensions
/// - Spawns ExtensionActors for each extension
/// - Produces HookRouter + ExtensionTool wrappers for agent-core integration
pub struct ExtensionManager {
    extensions: Vec<Arc<dyn Extension>>,
    event_bus_capacity: usize,
}

impl ExtensionManager {
    /// Create manager with ordered list of extensions.
    /// Order determines priority for blocking/chain hooks.
    pub fn new(extensions: Vec<Arc<dyn Extension>>) -> Self {
        Self {
            extensions,
            event_bus_capacity: 128,
        }
    }

    /// Collect all tool definitions. First-registration-wins (dedup by name).
    pub fn collect_tools(&self) -> Vec<ToolDef> {
        let mut seen = HashSet::new();
        let mut tools = Vec::new();
        for ext in &self.extensions {
            for tool in ext.tools() {
                if seen.insert(tool.name.clone()) {
                    tools.push(tool);
                }
            }
        }
        tools
    }

    /// Spawn all ExtensionActors.
    /// Returns:
    ///   - HookRouter: implements HookDispatcher, ready for agent-core
    ///   - ExtensionHandles: for constructing ExtensionTool wrappers
    ///   - JoinHandles: for graceful shutdown
    pub fn spawn_all(
        &self,
    ) -> (HookRouter, Vec<ExtensionHandle>, Vec<tokio::task::JoinHandle<()>>) {
        let event_bus = Arc::new(EventBus::new(self.event_bus_capacity));
        let mut handles = Vec::new();
        let mut join_handles = Vec::new();

        for ext in &self.extensions {
            let (handle, join_handle) = ExtensionActor::spawn(ext.clone(), event_bus.clone(), 32);
            handles.push(handle);
            join_handles.push(join_handle);
        }

        let hook_router = HookRouter::new(handles.clone(), event_bus);

        (hook_router, handles, join_handles)
    }

    /// Wrap extension tool definitions into AgentToolRef objects.
    ///
    /// Merge strategy: first-registration-wins per tool name.
    pub fn collect_agent_tools(
        &self,
        handles: &[ExtensionHandle],
    ) -> Vec<AgentToolRef> {
        let mut seen = HashSet::new();
        let mut tools = Vec::new();

        for (i, ext) in self.extensions.iter().enumerate() {
            if let Some(handle) = handles.get(i) {
                for tool_def in ext.tools() {
                    if seen.insert(tool_def.name.clone()) {
                        let execution_mode = ext
                            .tool_execution_modes()
                            .get(&tool_def.name)
                            .copied()
                            .unwrap_or_default();
                        tools.push(Arc::new(ExtensionTool {
                            name: tool_def.name,
                            description: tool_def.description,
                            parameters: tool_def.parameters,
                            handle: handle.clone(),
                            execution_mode,
                        }) as AgentToolRef);
                    }
                }
            }
        }
        tools
    }

    /// Shutdown all ExtensionActors gracefully.
    pub async fn shutdown_all(handles: &[ExtensionHandle]) {
        for handle in handles {
            handle.shutdown().await;
        }
    }
}