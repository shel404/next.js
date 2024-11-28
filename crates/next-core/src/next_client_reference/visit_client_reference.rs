use std::{collections::HashSet, future::Future};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use turbo_rcstr::RcStr;
use turbo_tasks::{
    debug::ValueDebugFormat,
    graph::{AdjacencyMap, GraphTraversal, Visit, VisitControlFlow},
    trace::TraceRawVcs,
    FxIndexMap, FxIndexSet, ReadRef, ResolvedVc, TryJoinIterExt, ValueToString, Vc,
};
use turbo_tasks_fs::FileSystemPath;
use turbopack::css::CssModuleAsset;
use turbopack_core::{module::Module, reference::primary_referenced_modules};

use super::ecmascript_client_reference::ecmascript_client_reference_module::EcmascriptClientReferenceModule;
use crate::next_server_component::server_component_module::NextServerComponentModule;

#[derive(
    Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Debug, ValueDebugFormat, TraceRawVcs,
)]
pub struct ClientReference {
    pub server_component: Option<ResolvedVc<NextServerComponentModule>>,
    pub ty: ClientReferenceType,
}

impl ClientReference {
    pub fn server_component(&self) -> Option<ResolvedVc<NextServerComponentModule>> {
        self.server_component
    }

    pub fn ty(&self) -> ClientReferenceType {
        self.ty
    }
}

#[derive(
    Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Debug, ValueDebugFormat, TraceRawVcs,
)]
pub enum ClientReferenceType {
    EcmascriptClientReference {
        module: ResolvedVc<EcmascriptClientReferenceModule>,
        /// should be the EcmascriptClientReferenceProxyModule
        parent_module: ResolvedVc<Box<dyn Module>>,
    },
    CssClientReference(ResolvedVc<CssModuleAsset>),
}

#[turbo_tasks::value(shared)]
#[derive(Clone, Debug)]
pub struct ClientReferenceGraphResult {
    pub client_references: Vec<ClientReference>,
    /// Only the [`ClientReferenceType::EcmascriptClientReference`]s are listed in this map.
    #[allow(clippy::type_complexity)]
    pub client_references_by_server_component: FxIndexMap<
        Option<ResolvedVc<NextServerComponentModule>>,
        Vec<ResolvedVc<EcmascriptClientReferenceModule>>,
    >,
    pub server_component_entries: Vec<ResolvedVc<NextServerComponentModule>>,
    pub server_utils: Vec<ResolvedVc<Box<dyn Module>>>,
    pub visited_nodes: ResolvedVc<VisitedClientReferenceGraphNodes>,
}

impl ClientReferenceGraphResult {
    /// Merges multiple return values of client_reference_graph together.
    pub fn extend(&mut self, other: &Self) {
        self.client_references
            .extend(other.client_references.iter().copied());
        for (k, v) in other.client_references_by_server_component.iter() {
            self.client_references_by_server_component
                .entry(*k)
                .or_insert_with(Vec::new)
                .extend(v);
        }
        self.server_component_entries
            .extend(other.server_component_entries.iter().copied());
        self.server_utils.extend(other.server_utils.iter().copied());
        // This is merged already by `client_reference_graph` itself
        self.visited_nodes = other.visited_nodes;
    }
}

impl Default for ClientReferenceGraphResult {
    fn default() -> Self {
        ClientReferenceGraphResult {
            client_references: Default::default(),
            client_references_by_server_component: Default::default(),
            server_component_entries: Default::default(),
            server_utils: Default::default(),
            visited_nodes: VisitedClientReferenceGraphNodes(Default::default()).resolved_cell(),
        }
    }
}

#[turbo_tasks::value(shared)]
pub struct VisitedClientReferenceGraphNodes(HashSet<VisitClientReferenceNode>);

#[turbo_tasks::value_impl]
impl VisitedClientReferenceGraphNodes {
    #[turbo_tasks::function]
    pub fn empty() -> Vc<Self> {
        VisitedClientReferenceGraphNodes(Default::default()).cell()
    }
}

#[turbo_tasks::value(transparent)]
pub struct ClientReferenceTypes(FxIndexSet<ClientReferenceType>);

#[turbo_tasks::value_impl]
impl ClientReferenceGraphResult {
    #[turbo_tasks::function]
    pub fn types(&self) -> Vc<ClientReferenceTypes> {
        Vc::cell(
            self.client_references
                .iter()
                .map(|r| r.ty())
                .collect::<FxIndexSet<_>>(),
        )
    }
}

#[turbo_tasks::value(shared)]
#[derive(Clone, Debug)]
pub struct ServerEntries {
    pub server_component_entries: Vec<ResolvedVc<NextServerComponentModule>>,
    pub server_utils: Vec<ResolvedVc<Box<dyn Module>>>,
}

#[turbo_tasks::function]
pub async fn find_server_entries(entry: ResolvedVc<Box<dyn Module>>) -> Result<Vc<ServerEntries>> {
    let graph = AdjacencyMap::new()
        .skip_duplicates()
        .visit(
            vec![VisitClientReferenceNode {
                state: {
                    VisitClientReferenceNodeState::Entry {
                        entry_path: entry.ident().path().resolve().await?,
                    }
                },
                ty: VisitClientReferenceNodeType::Internal(entry, entry.ident().to_string().await?),
            }],
            VisitClientReference {},
        )
        .await
        .completed()?
        .into_inner();

    let mut server_component_entries = vec![];
    let mut server_utils = vec![];
    for node in graph.reverse_topological() {
        match &node.ty {
            VisitClientReferenceNodeType::ServerUtilEntry(server_util, _) => {
                server_utils.push(*server_util);
            }
            VisitClientReferenceNodeType::ServerComponentEntry(server_component, _) => {
                server_component_entries.push(*server_component);
            }
            VisitClientReferenceNodeType::Internal(_, _)
            | VisitClientReferenceNodeType::ClientReference(_, _) => {}
        }
    }

    Ok(ServerEntries {
        server_component_entries,
        server_utils,
    }
    .cell())
}

struct VisitClientReference {}

#[derive(
    Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Debug, ValueDebugFormat, TraceRawVcs,
)]
struct VisitClientReferenceNode {
    state: VisitClientReferenceNodeState,
    ty: VisitClientReferenceNodeType,
}

#[derive(
    Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Debug, ValueDebugFormat, TraceRawVcs,
)]
enum VisitClientReferenceNodeState {
    Entry {
        entry_path: Vc<FileSystemPath>,
    },
    InServerComponent {
        server_component: Vc<NextServerComponentModule>,
    },
    InServerUtil,
}
impl VisitClientReferenceNodeState {
    fn server_component(&self) -> Option<Vc<NextServerComponentModule>> {
        match self {
            VisitClientReferenceNodeState::Entry { .. } => None,
            VisitClientReferenceNodeState::InServerComponent { server_component } => {
                Some(*server_component)
            }
            VisitClientReferenceNodeState::InServerUtil => None,
        }
    }
}

#[derive(
    Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Debug, ValueDebugFormat, TraceRawVcs,
)]
enum VisitClientReferenceNodeType {
    ClientReference(ClientReference, ReadRef<RcStr>),
    ServerComponentEntry(ResolvedVc<NextServerComponentModule>, ReadRef<RcStr>),
    ServerUtilEntry(ResolvedVc<Box<dyn Module>>, ReadRef<RcStr>),
    Internal(ResolvedVc<Box<dyn Module>>, ReadRef<RcStr>),
}

impl Visit<VisitClientReferenceNode> for VisitClientReference {
    type Edge = VisitClientReferenceNode;
    type EdgesIntoIter = Vec<Self::Edge>;
    type EdgesFuture = impl Future<Output = Result<Self::EdgesIntoIter>>;

    fn visit(&mut self, edge: Self::Edge) -> VisitControlFlow<VisitClientReferenceNode> {
        match edge.ty {
            VisitClientReferenceNodeType::Internal(..) => VisitControlFlow::Continue(edge),
            VisitClientReferenceNodeType::ClientReference(..)
            | VisitClientReferenceNodeType::ServerUtilEntry(..)
            | VisitClientReferenceNodeType::ServerComponentEntry(..) => {
                VisitControlFlow::Skip(edge)
            }
        }
    }

    fn edges(&mut self, node: &VisitClientReferenceNode) -> Self::EdgesFuture {
        let node = node.clone();
        async move {
            let parent_module = match node.ty {
                // This should never occur since we always skip visiting these
                // nodes' edges.
                VisitClientReferenceNodeType::ClientReference(..) => return Ok(vec![]),
                VisitClientReferenceNodeType::Internal(module, _) => module,
                VisitClientReferenceNodeType::ServerUtilEntry(module, _) => module,
                VisitClientReferenceNodeType::ServerComponentEntry(module, _) => {
                    ResolvedVc::upcast(module)
                }
            };

            let referenced_modules = primary_referenced_modules(*parent_module).await?;

            let referenced_modules = referenced_modules.iter().map(|module| async move {
                let module = module.to_resolved().await?;
                if let Some(client_reference_module) =
                    ResolvedVc::try_downcast_type::<EcmascriptClientReferenceModule>(module).await?
                {
                    return Ok(VisitClientReferenceNode {
                        state: node.state,
                        ty: VisitClientReferenceNodeType::ClientReference(
                            ClientReference {
                                server_component: match node.state.server_component() {
                                    Some(server_component) => {
                                        Some(server_component.to_resolved().await?)
                                    }
                                    None => None,
                                },
                                ty: ClientReferenceType::EcmascriptClientReference {
                                    parent_module,
                                    module: client_reference_module,
                                },
                            },
                            client_reference_module.ident().to_string().await?,
                        ),
                    });
                }

                if let Some(css_client_reference_asset) =
                    ResolvedVc::try_downcast_type::<CssModuleAsset>(module).await?
                {
                    return Ok(VisitClientReferenceNode {
                        state: node.state,
                        ty: VisitClientReferenceNodeType::ClientReference(
                            ClientReference {
                                server_component: match node.state.server_component() {
                                    Some(server_component) => {
                                        Some(server_component.to_resolved().await?)
                                    }
                                    None => None,
                                },
                                ty: ClientReferenceType::CssClientReference(
                                    css_client_reference_asset,
                                ),
                            },
                            css_client_reference_asset.ident().to_string().await?,
                        ),
                    });
                }

                if let Some(server_component_asset) =
                    ResolvedVc::try_downcast_type::<NextServerComponentModule>(module).await?
                {
                    return Ok(VisitClientReferenceNode {
                        state: VisitClientReferenceNodeState::InServerComponent {
                            server_component: *server_component_asset,
                        },
                        ty: VisitClientReferenceNodeType::ServerComponentEntry(
                            server_component_asset,
                            server_component_asset.ident().to_string().await?,
                        ),
                    });
                }

                if let VisitClientReferenceNodeState::Entry { entry_path } = &node.state {
                    if module.ident().path().resolve().await? != *entry_path {
                        return Ok(VisitClientReferenceNode {
                            state: VisitClientReferenceNodeState::InServerUtil,
                            ty: VisitClientReferenceNodeType::ServerUtilEntry(
                                module,
                                module.ident().to_string().await?,
                            ),
                        });
                    }
                }

                Ok(VisitClientReferenceNode {
                    state: node.state,
                    ty: VisitClientReferenceNodeType::Internal(
                        module,
                        module.ident().to_string().await?,
                    ),
                })
            });

            let assets = referenced_modules.try_join().await?;

            Ok(assets)
        }
    }

    fn span(&mut self, node: &VisitClientReferenceNode) -> tracing::Span {
        match &node.ty {
            VisitClientReferenceNodeType::ClientReference(_, name) => {
                tracing::info_span!("client reference", name = name.to_string())
            }
            VisitClientReferenceNodeType::Internal(_, name) => {
                tracing::info_span!("module", name = name.to_string())
            }
            VisitClientReferenceNodeType::ServerUtilEntry(_, name) => {
                tracing::info_span!("server util", name = name.to_string())
            }
            VisitClientReferenceNodeType::ServerComponentEntry(_, name) => {
                tracing::info_span!("layout segment", name = name.to_string())
            }
        }
    }
}
