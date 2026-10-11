import { findLastCompactionTimestamp } from "../../node_modules/@tarquinen/opencode-dcp/lib/state/utils.ts";
import { syncCompressionBlocks } from "../../node_modules/@tarquinen/opencode-dcp/lib/messages/sync.ts";

// A host compactor may preserve only part of a compressed range. Never prune
// that suffix unless every source member and the summary injection anchor still
// exist. Validation uses canonical IDs, not transient upstream message refs.
export function reconcileHistory(state, messages, logger) {
  const current = new Map(messages.map((message) => [message.info.id, message]));
  const tools = new Set(messages.flatMap((message) => message.parts.filter((part) => part.type === "tool").map((part) => part.callID)));
  const prune = state.prune.messages;
  const invalid = new Set();
  for (const [id, block] of prune.blocksById) {
    const source = current.get(block.compressMessageId);
    const hasCall = source?.parts.some((part) => part.type === "tool" && part.tool === "compress" && part.callID === block.compressCallId);
    const members = block.effectiveMessageIds;
    const valid = hasCall && current.has(block.anchorMessageId) && block.summary.trim().length > 0 &&
      members.length > 0 && members.includes(block.anchorMessageId) &&
      members.every((messageId) => current.has(messageId) && prune.byMessageId.get(messageId)?.allBlockIds.includes(id)) &&
      block.effectiveToolIds.every((toolId) => tools.has(toolId));
    if (!valid) invalid.add(id);
  }
  // Included/consumed blocks provide content to their parents. Invalidation
  // flows upwards; a valid child must survive when only its parent is invalid.
  let changed;
  do {
    changed = false;
    for (const [id, block] of prune.blocksById) {
      if (invalid.has(id)) continue;
      const dependencies = [...block.includedBlockIds, ...block.consumedBlockIds];
      if (dependencies.some((childId) => childId >= id || !prune.blocksById.has(childId) || invalid.has(childId))) {
        invalid.add(id);
        changed = true;
      }
    }
  } while (changed);
  for (const id of invalid) prune.blocksById.delete(id);
  for (const block of prune.blocksById.values()) {
    block.parentBlockIds = block.parentBlockIds.filter((id) => prune.blocksById.get(id)?.consumedBlockIds.includes(block.blockId));
    if (block.deactivatedByBlockId && !prune.blocksById.has(block.deactivatedByBlockId)) block.deactivatedByBlockId = undefined;
  }
  for (const [messageId, entry] of prune.byMessageId) {
    entry.allBlockIds = [...new Set(entry.allBlockIds)].filter((id) => prune.blocksById.get(id)?.effectiveMessageIds.includes(messageId));
    if (!current.has(messageId) || !entry.allBlockIds.length) prune.byMessageId.delete(messageId);
  }
  // Upstream rebuilds activation/consumption maps from the remaining graph.
  prune.activeBlockIds.clear();
  prune.activeByAnchorMessageId.clear();
  syncCompressionBlocks(state, logger, messages);
  for (const [id] of state.prune.tools) if (!tools.has(id)) state.prune.tools.delete(id);
  state.lastCompaction = findLastCompactionTimestamp(messages);
  // Tool-cache turn positions are derived from the current projected history.
  // Retaining a call ID/status/input does not retain its age after a compactor
  // replaces the prefix; upstream syncToolCache skips existing cache entries.
  state.toolParameters.clear();
  state.subAgentResultCache.clear();
  state.toolIdList = [];
  for (const anchors of Object.values(state.nudges)) for (const id of anchors) if (!current.has(id)) anchors.delete(id);
  // Synthetic projection timestamps are positions, not host checkpoint clocks.
  // Derive the current boundary before checkSession instead of comparing an
  // ephemeral checkpointId and globally resetting warm-only persisted state.
  state.messageIds = { byRawId: new Map(), byRef: new Map(), nextRef: 1 };
  if (invalid.size) logger.info("Invalidated DCP blocks after host history replacement", { blockIds: [...invalid] });
}
