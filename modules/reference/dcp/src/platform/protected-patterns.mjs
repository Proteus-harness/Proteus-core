// Adapt only the shipped Proteus file DTOs, solely inside protection checks.
// Names and arguments in projected/restored requests remain canonical.
import { getFilePathsFromParameters as upstreamPaths, isFilePathProtected, isToolNameProtected } from "../../node_modules/@tarquinen/opencode-dcp/lib/protected-patterns.ts";
export { matchesGlob, isFilePathProtected, isToolNameProtected } from "../../node_modules/@tarquinen/opencode-dcp/lib/protected-patterns.ts";

export function getFilePathsFromParameters(tool, parameters) {
  if (!parameters || typeof parameters !== "object") return upstreamPaths(tool, parameters);
  if (["read_file", "write_file", "edit_file"].includes(tool) && typeof parameters.path === "string") {
    return upstreamPaths(tool, { ...parameters, filePath: parameters.path });
  }
  if (tool === "read_many_files" && Array.isArray(parameters.paths)) {
    return [...new Set(parameters.paths.filter((path) => typeof path === "string" && path.length > 0))];
  }
  if (tool === "apply_patch") {
    const patch = typeof parameters.patch === "string" ? parameters.patch : parameters.input;
    if (typeof patch === "string") {
      const paths = upstreamPaths(tool, { ...parameters, patchText: patch });
      // Proteus patch DTOs include a destination in rename operations.
      const destinations = [...patch.matchAll(/^\*\*\* Move to: ([^\n\r]+)/gm)].map((match) => match[1].trim());
      return [...new Set([...paths, ...destinations])].filter(Boolean);
    }
  }
  return upstreamPaths(tool, parameters);
}

export function isToolProtected(tool, input, tools, files, metadata) {
  const calls = [{ tool, input }];
  if (tool === "execute" && Array.isArray(metadata?.toolCalls)) {
    for (const call of metadata.toolCalls) if (call && typeof call.tool === "string") calls.push(call);
  }
  return calls.some((call) => isToolNameProtected(call.tool, tools) ||
    isFilePathProtected(getFilePathsFromParameters(call.tool, call.input), files));
}
