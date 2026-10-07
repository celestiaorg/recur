package agenttools

// HostTools is Recur's first-class host tool set. MCP HTTP wraps Execute;
// the native harness calls the same tools in-process. Coding tools are not included.
func HostTools() []Tool {
	return []Tool{
		reportFindingTool{},
		listFindingsTool{},
		updateFindingTool{},
		doneTool{},
		reportResultTool{},
		planTool{},
		escalateTool{},
		attachEvidenceTool{},
		editPlanTool{},
		rerunReviewTool{},
		dispatchSpecialistTool{},
		extendIterationsTool{},
		extendRoundsTool{},
		reenterPhaseTool{},
		runPhaseTool{},
		resumeLoopTool{},
		submitPlanTool{},
		updatePlanStepTool{},
		sendBackTool{},
		permissionRequestTool{},
		listSkillsTool{},
		activateSkillTool{},
		askUserTool{},
		switchModeTool{},
		createArtifactTool{},
		listArtifactsTool{},
		readArtifactTool{},
		presentArtifactTool{},
		listWorkspaceBriefsTool{},
		getWorkspaceContextTool{},
		presentWorkspaceTool{},
		renameSessionTool{},
		renameWorkspaceTool{},
		renameBranchTool{},
		openPrTool{},
		createWorkspaceTool{},
		mergeChildTool{},
		readChildTool{},
		archiveChildTool{},
		messageChildTool{},
		listTerminalsTool{},
		readTerminalTool{},
	}
}

var (
	_ HaltTool = askUserTool{}
	_ HaltTool = switchModeTool{}
)
