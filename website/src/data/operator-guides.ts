/**
 * Presentation metadata for repository guides rendered from `docs/*.md`.
 * Page content always comes from the versioned Markdown file; a contract test
 * requires every guide in `docs/` to appear here exactly once.
 */
export type OperatorGuide = {
  slug: string;
  label: string;
  description: string;
};

export type OperatorGuideGroup = {
  label: string;
  guides: readonly OperatorGuide[];
};

export const OPERATOR_GUIDE_GROUPS = [
  {
    label: "Inspect and diagnose",
    guides: [
      { slug: "mock-stdio", label: "Interactive stdio walkthrough", description: "Run the synthetic mock Core, generate an MCP Inspector configuration, and troubleshoot startup with safe diagnostics." },
      { slug: "diagnostics", label: "Diagnostic tools", description: "Fleet health, incident history, bounded workload logs, and evidence-backed stack diagnosis." },
      { slug: "docker", label: "Docker objects", description: "Inspect images, networks, and volumes on one Server with safe history and explicit cache limits." },
      { slug: "swarms", label: "Swarm inspection", description: "Inspect existing Swarms, nodes, services, and tasks, trace placement failures, and read bounded service logs." },
      { slug: "export", label: "Scoped TOML export", description: "Export explicit resources or groups as sanitized, incomplete inspection TOML using only Core reads." },
    ],
  },
  {
    label: "Operate and recover",
    guides: [
      { slug: "operations", label: "Workload operations", description: "Deploy, start, stop, or restart one workload, wait by update ID, and reconcile interrupted submissions without repeating execution." },
      { slug: "operations-walkthrough", label: "Operation walkthrough", description: "Inspect, operate, wait, resume, and independently verify health with the compiled stdio server." },
      { slug: "scaling", label: "Swarm scaling", description: "Preview and save replica counts, deploy the saved definition explicitly, and observe tasks separately." },
      { slug: "pruning", label: "Docker pruning", description: "Preview and apply one reviewed Docker category prune on one Server behind a separate default-off grant." },
      { slug: "terminal", label: "One terminal command", description: "Run one bounded command in a fresh private terminal behind its own default-off grant." },
    ],
  },
  {
    label: "Configure definitions",
    guides: [
      { slug: "configuration", label: "Typed configuration edits", description: "The shared preview/apply contract and Stack definition fields, including stale previews and uncertain submissions." },
      { slug: "deployments", label: "Deployment definitions", description: "Typed Deployment fields, Server and Swarm targets, Build versions, and separate runtime observation." },
      { slug: "servers", label: "Server records", description: "Server connection and alert settings, explicit initial trust, and separately observed connectivity." },
      { slug: "repositories", label: "Repositories", description: "Repo definitions, hook disclosure, and explicit clone or pull with receipts and recovery." },
      { slug: "builds", label: "Builders and Builds", description: "Builder and Build definitions plus explicit run and cancel of one saved Build." },
      { slug: "procedures", label: "Trusted Procedures", description: "Ordered stages, explicit saved runs, cancellation, and separate parent and child outcomes." },
      { slug: "actions", label: "Trusted Actions", description: "Private Action source previews, inert creation, and explicit run and cancel with delegated authority." },
      { slug: "schedules", label: "Schedules", description: "Inspect and update Core-owned Action and Procedure schedules while preserving the parent definition." },
      { slug: "alerters", label: "Alerters", description: "Inspect Alerters without endpoint secrets, preview changes privately, and send one explicit test." },
      { slug: "variables", label: "Variables and secrets", description: "Variable metadata without secret values, configured Core secret names, and typed variable changes." },
      { slug: "tags", label: "Tags", description: "Tag creation, rename, exact assignment, discovery by tag, and reviewed deletion." },
      { slug: "deletion", label: "Deliberate deletion", description: "Preview and apply deletion of one exact Stack, Deployment, or Server definition." },
    ],
  },
  {
    label: "ResourceSync",
    guides: [
      { slug: "resource-syncs", label: "ResourceSync definitions", description: "Define ResourceSyncs, inspect and refresh pending plans, review a selection, and apply it once." },
      { slug: "resource-sync-walkthrough", label: "ResourceSync walkthrough", description: "Export, define, refresh, review, apply, and independently inspect ResourceSync desired state." },
      { slug: "automation-walkthrough", label: "Automation walkthrough", description: "Follow a revision through Builds, images, deployments, trusted authority, schedules, and recovery." },
    ],
  },
  {
    label: "Administration",
    guides: [
      { slug: "access", label: "Users, groups and access", description: "Inspect users, groups, grants, and effective permissions, and apply one scoped administration change." },
      { slug: "swarm-admin-walkthrough", label: "Swarm and administration walkthrough", description: "The complete Swarm and administration operator workflow with opt-in manager and worker acceptance." },
    ],
  },
  {
    label: "Project",
    guides: [
      { slug: "container", label: "Docker Compose installation", description: "Run the multi-platform image with runtime credentials, verify the authenticated listener, and find matching source and notices." },
      { slug: "distribution", label: "Native binaries and matching source", description: "Verify a native archive, check its platform requirements, and rebuild from the bundled source and dependencies." },
      { slug: "maintenance", label: "Repository maintenance", description: "Dependency updates, required checks, main-branch protection, and release compatibility." },
    ],
  },
] as const satisfies readonly OperatorGuideGroup[];

export const OPERATOR_GUIDES: readonly OperatorGuide[] = OPERATOR_GUIDE_GROUPS.flatMap(
  ({ guides }): readonly OperatorGuide[] => guides,
);

export function operatorGuideRoute(slug: string) {
  return `/docs/${slug}/`;
}
