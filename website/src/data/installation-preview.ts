/** The documentation review deploy can install the exact packaged main candidate before a release exists. */
export const REVIEW_CANDIDATE = {
  revision: "097ae18954992724f541f9e9e250a1e9136c06b1",
  version: "0.9.0",
  nativeRun: "45898798181",
  containerRun: "45898798198",
  checksumsSha256: "86e17fe501225ea965e0b6ded8c4e237000f033c14fe4b3509e3666b7f309e43",
  archiveSha256: "216533a21f2a09f3c51c6129ba11a94f2b0265dc89ae884608cad47b5a0bf3cb",
};

export function installationCandidate(env: Readonly<Record<string, string | undefined>>) {
  return env.CF_PAGES === "1" && env.CF_PAGES_BRANCH === "antoine/stable-operator-docs"
    ? REVIEW_CANDIDATE
    : undefined;
}

export const previewCandidate = installationCandidate(process.env);
