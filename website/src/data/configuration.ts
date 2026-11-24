export type ConfigurationVariable = {
  name: string;
  required: boolean;
  /** Default when unset; `undefined` for required values. */
  defaultValue?: "false";
  placeholder: string;
  summary: string;
};

/**
 * Environment read once by the komodo-mcp process at startup.
 * Contract tests check these names and defaults against `.env.example` and `src/config.rs`.
 */
export const CORE_CONNECTION_VARIABLES = [
  {
    name: "KOMODO_ADDRESS",
    required: true,
    placeholder: "https://komodo.example.test",
    summary:
      "Absolute HTTP or HTTPS address of Komodo Core, optionally with a base path. User information, query strings, and fragments are rejected; redirects are not followed.",
  },
  {
    name: "KOMODO_API_KEY",
    required: true,
    placeholder: "replace-with-your-api-key",
    summary: "API key of the enabled Core identity whose permissions every call uses.",
  },
  {
    name: "KOMODO_API_SECRET",
    required: true,
    placeholder: "replace-with-your-api-secret",
    summary: "API secret paired with that key. Never shown in output, logs, or Debug formatting.",
  },
] as const satisfies readonly ConfigurationVariable[];

export const CAPABILITY_VARIABLES = [
  {
    name: "KOMODO_ALLOW_OPERATE",
    required: false,
    defaultValue: "false",
    placeholder: "false",
    summary:
      "Single-workload operations, explicit Repo clone/pull, and saved Build, Procedure, and Action runs. Core Execute is still required.",
  },
  {
    name: "KOMODO_ALLOW_CONFIGURATION_EDIT",
    required: false,
    defaultValue: "false",
    placeholder: "false",
    summary:
      "Typed configuration preview/apply tools. Enabling it never enables operations; some applies require both grants.",
  },
  {
    name: "KOMODO_ALLOW_PRUNE",
    required: false,
    defaultValue: "false",
    placeholder: "false",
    summary:
      "Docker category prune preview/apply on one Server. Also requires the operate grant and Core Execute.",
  },
  {
    name: "KOMODO_ALLOW_TERMINAL",
    required: false,
    defaultValue: "false",
    placeholder: "false",
    summary:
      "One command in a fresh private terminal. Also requires the operate grant and Core Read + Terminal.",
  },
] as const satisfies readonly ConfigurationVariable[];

export const CONFIGURATION_VARIABLES = [
  ...CORE_CONNECTION_VARIABLES,
  ...CAPABILITY_VARIABLES,
  { name: "KOMODO_TRANSPORT" },
  { name: "KOMODO_HTTP_BIND" },
  { name: "KOMODO_HTTP_TOKENS" },
  { name: "KOMODO_HTTP_ALLOWED_ORIGINS" },
] as const;
