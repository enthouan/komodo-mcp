export type ClientKey = "codex" | "claude-code" | "claude-desktop" | "vscode" | "opencode";
export type ClientIcon = "openai" | "claude" | "vscode" | "opencode";
export type RecipeLanguage = "json" | "jsonc" | "toml";

export interface ClientSetup {
  key: ClientKey;
  icon: ClientIcon;
  label: string;
  description: string;
  configLocation: string;
  configTitle: string;
  language: RecipeLanguage;
  code: string;
  reload: string;
  secretNote: string;
  docsLabel: string;
  docsUrl: string;
  /** Date the configuration syntax was last compared with the client's documentation. */
  reviewed: string;
}

/** Placeholder for the compiled executable; clients need its absolute path. */
export const EXECUTABLE_PLACEHOLDER = "/absolute/path/to/komodo-mcp/target/release/komodo-mcp";

export const VERIFY_PROMPT =
  "List the stacks I can access in Komodo and show their current status. Use list_stacks and do not change anything.";

export const VERIFY_EXPECTED =
  "Approve only list_stacks. It is read-only, needs no arguments, and returns one page of the Stacks your Core identity can read, each with its stable ID, state, and active target. Follow next_page with the same page_size until has_more is false. An empty list is a successful result when that identity can read no Stacks.";

/** Actual structuredContent returned by list_stacks({"page_size": 2}) from the repository's synthetic mock Core. */
export const VERIFY_EXAMPLE = `{
  "items": [
    {
      "id": "000000000000000000000002",
      "name": "alpha-web",
      "template": false,
      "state": "running",
      "target": {
        "kind": "server",
        "id": "000000000000000000000001",
        "name": "fixture-server"
      },
      "service_count": 2
    },
    {
      "id": "000000000000000000000004",
      "name": "beta-other",
      "template": false,
      "state": "running",
      "target": {
        "kind": "server",
        "id": "000000000000000000000009",
        "name": "other-server"
      },
      "service_count": 2
    }
  ],
  "page": 0,
  "page_size": 2,
  "has_more": true,
  "next_page": 1
}`;

const READ_ONLY_GRANTS = [
  "KOMODO_ALLOW_OPERATE",
  "KOMODO_ALLOW_CONFIGURATION_EDIT",
  "KOMODO_ALLOW_PRUNE",
  "KOMODO_ALLOW_TERMINAL",
] as const;

const REVIEWED = "2026-11-16";

function jsonEnvironment(values: Record<string, string>, indent: string) {
  return Object.entries(values)
    .map(([name, value]) => `${indent}"${name}": "${value}"`)
    .join(",\n");
}

function readOnlyGrants(): Record<string, string> {
  return Object.fromEntries(READ_ONLY_GRANTS.map((name) => [name, "false"]));
}

export const CLIENT_SETUPS = [
  {
    key: "codex",
    icon: "openai",
    label: "Codex",
    description:
      "Codex reads MCP servers from config.toml. env_vars forwards the already exported Core connection values without writing them into the file.",
    configLocation: "Add these tables to ~/.codex/config.toml (or .codex/config.toml in a trusted project).",
    configTitle: "~/.codex/config.toml",
    language: "toml",
    code: `[mcp_servers.komodo]
command = "${EXECUTABLE_PLACEHOLDER}"
env_vars = ["KOMODO_ADDRESS", "KOMODO_API_KEY", "KOMODO_API_SECRET"]

[mcp_servers.komodo.env]
${READ_ONLY_GRANTS.map((name) => `${name} = "false"`).join("\n")}`,
    reload:
      "Start a new Codex session from the shell that exported the Core variables, then run codex mcp list or /mcp to confirm that komodo is listed.",
    secretNote:
      "Export KOMODO_ADDRESS, KOMODO_API_KEY, and KOMODO_API_SECRET before launching Codex. The TOML stores only their names.",
    docsLabel: "Codex MCP documentation",
    docsUrl: "https://learn.chatgpt.com/docs/extend/mcp",
    reviewed: REVIEWED,
  },
  {
    key: "claude-code",
    icon: "claude",
    label: "Claude Code",
    description:
      "Claude Code expands ${VAR} references in a project-scoped .mcp.json, so the shared file names the Core credentials without containing them.",
    configLocation:
      "Export the three Core variables, then create .mcp.json at the project root. Claude Code asks you to approve project-scoped servers.",
    configTitle: ".mcp.json",
    language: "json",
    code: `{
  "mcpServers": {
    "komodo": {
      "type": "stdio",
      "command": "${EXECUTABLE_PLACEHOLDER}",
      "args": [],
      "env": {
${jsonEnvironment(
  {
    KOMODO_ADDRESS: "${KOMODO_ADDRESS}",
    KOMODO_API_KEY: "${KOMODO_API_KEY}",
    KOMODO_API_SECRET: "${KOMODO_API_SECRET}",
    ...readOnlyGrants(),
  },
  "        ",
)}
      }
    }
  }
}`,
    reload:
      "Start a new Claude Code session, approve the project server, then run /mcp or claude mcp list and confirm that komodo is connected.",
    secretNote:
      "The file contains only environment-variable references, so it can be shared after review. Claude Code keeps an unset reference as literal text, which makes startup fail; export all three values first.",
    docsLabel: "Claude Code MCP documentation",
    docsUrl: "https://code.claude.com/docs/en/mcp",
    reviewed: REVIEWED,
  },
  {
    key: "claude-desktop",
    icon: "claude",
    label: "Claude Desktop",
    description:
      "Claude Desktop launches the compiled executable as a developer-defined local stdio server. komodo-mcp does not ship a desktop extension.",
    configLocation:
      "Open Settings → Developer → Edit Config and merge this entry into claude_desktop_config.json without removing other servers.",
    configTitle: "claude_desktop_config.json",
    language: "json",
    code: `{
  "mcpServers": {
    "komodo": {
      "command": "${EXECUTABLE_PLACEHOLDER}",
      "args": [],
      "env": {
${jsonEnvironment(
  {
    KOMODO_ADDRESS: "https://komodo.example.test",
    KOMODO_API_KEY: "replace-in-this-user-local-file",
    KOMODO_API_SECRET: "replace-in-this-user-local-file",
    ...readOnlyGrants(),
  },
  "        ",
)}
      }
    }
  }
}`,
    reload:
      "Fully quit and reopen Claude Desktop, then open Connectors → Manage connectors and select komodo to see its tools. Server stderr is written to mcp-server-komodo.log in Claude's log directory.",
    secretNote:
      "This user-local file holds the Core API key and secret in plaintext. Restrict access to it and never copy it into a repository or support request.",
    docsLabel: "MCP local-server guide",
    docsUrl: "https://modelcontextprotocol.io/docs/develop/connect-local-servers",
    reviewed: REVIEWED,
  },
  {
    key: "vscode",
    icon: "vscode",
    label: "VS Code",
    description:
      "VS Code's Copilot harness reads the portable mcpServers format and expands ${NAME} references in a stdio server's command, args, and env.",
    configLocation:
      "Merge this entry into ~/.copilot/mcp-config.json ($COPILOT_HOME/mcp-config.json when set), or add it with MCP: Add Server → Global.",
    configTitle: "~/.copilot/mcp-config.json",
    language: "json",
    code: `{
  "mcpServers": {
    "komodo": {
      "type": "stdio",
      "command": "${EXECUTABLE_PLACEHOLDER}",
      "args": [],
      "env": {
${jsonEnvironment(
  {
    KOMODO_ADDRESS: "${KOMODO_ADDRESS}",
    KOMODO_API_KEY: "${KOMODO_API_KEY}",
    KOMODO_API_SECRET: "${KOMODO_API_SECRET}",
    ...readOnlyGrants(),
  },
  "        ",
)}
      }
    }
  }
}`,
    reload:
      "Launch VS Code from an environment that defines the Core variables, then run MCP: List Servers and start komodo. Adding configuration alone does not confirm that a chat session can use it; run the verification call.",
    secretNote:
      "The portable file does not support ${input:…} prompts, so keep the values in the environment rather than in the file. The older editor-managed servers format in mcp.json is deprecated by VS Code.",
    docsLabel: "VS Code MCP configuration reference",
    docsUrl: "https://code.visualstudio.com/docs/agents/reference/mcp-configuration",
    reviewed: REVIEWED,
  },
  {
    key: "opencode",
    icon: "opencode",
    label: "OpenCode",
    description:
      "OpenCode starts local servers from a command array and substitutes {env:NAME} inside JSON strings.",
    configLocation:
      "Merge this into the project's opencode.jsonc, or add the server with opencode mcp add --global for every project.",
    configTitle: "opencode.jsonc",
    language: "jsonc",
    code: `{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "servers": {
      "komodo": {
        "type": "local",
        "command": ["${EXECUTABLE_PLACEHOLDER}"],
        "environment": {
${jsonEnvironment(
  {
    KOMODO_ADDRESS: "{env:KOMODO_ADDRESS}",
    KOMODO_API_KEY: "{env:KOMODO_API_KEY}",
    KOMODO_API_SECRET: "{env:KOMODO_API_SECRET}",
    ...readOnlyGrants(),
  },
  "          ",
)}
        }
      }
    }
  }
}`,
    reload:
      "Restart OpenCode from the shell that exported the Core variables and confirm that the komodo server is connected and its tools are listed.",
    secretNote:
      "Shell-style $NAME is not expanded in OpenCode JSON strings; use the {env:NAME} form shown so the file never contains the credentials.",
    docsLabel: "OpenCode MCP server documentation",
    docsUrl: "https://opencode.ai/docs/mcp-servers/",
    reviewed: REVIEWED,
  },
] as const satisfies readonly ClientSetup[];
