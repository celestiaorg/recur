

/**
 * Claude Code's background-launch ack is instructions for the MODEL
 * (agentId, SendMessage, output_file). Prefix-match so a Read of a file that
 * merely mentions the phrase is not treated as a launch ack. Keep in sync
 * with engine isSubagentLaunchMeta.
 */
export function isSubagentLaunchAck(result: string): boolean {
  return result.trim().startsWith('Async agent launched successfully')
}
