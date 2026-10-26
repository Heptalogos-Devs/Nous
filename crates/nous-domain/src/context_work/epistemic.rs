use super::*;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEpistemic {
    pub stance: Option<EpistemicStance>,
    pub cognitive_role: Option<CognitiveRole>,
    pub grounding_kind: Option<ObservationKind>,
    pub source_event: Option<ObservationId>,
    pub qualifications: Vec<String>,
    pub uncertain: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextEpistemicView {
    pub accepted_facts_and_reports: Vec<String>,
    pub experiences: Vec<String>,
    pub execution_assistance: Vec<String>,
    pub unknowns: Vec<String>,
    pub familiar_domains: Vec<String>,
    pub learning_opportunities: Vec<String>,
    pub coverage: Vec<String>,
}
