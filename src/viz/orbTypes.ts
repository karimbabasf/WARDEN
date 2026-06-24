export type OrbEvidence = {
  session_id: string;
  turn_id?: string | null;
  event_id?: string | null;
  quote?: string | null;
  source_path?: string | null;
};

export type OrbAgent = {
  id: string;
  harness: string;
  label: string;
  glyph: string;
  color: string;
  sessions: number;
  eventCount: number;
  totalLoad: number;
};

export type OrbIssue = {
  id: string;
  agentId: string;
  harness: string;
  patternId: string;
  title: string;
  count: number;
  severity: number;
  rationale: string;
  estCostTokens: number;
  estCostMinutes: number;
  frequency: number;
  confidence: number;
  sessionIds: string[];
  evidence: OrbEvidence[];
  findingId?: string;
  verifierVerdict?: string;
  status?: string;
};

export type OrbLink = {
  source: string;
  target: string;
  kind: 'agent_issue';
};

export type OrbGuidance = {
  doItems: string[];
  stopItems: string[];
};

export type OrbSceneModel = {
  agents: OrbAgent[];
  issues: OrbIssue[];
  links: OrbLink[];
  guidance: OrbGuidance;
};

export type Vec3 = { x: number; y: number; z: number };

export type LayoutNodeKind = 'hub' | 'issue';

export type LayoutNode = {
  id: string;
  kind: LayoutNodeKind;
  position: Vec3;
  radius: number;
  agentId: string;
  harness: string;
  issue?: OrbIssue;
  agent?: OrbAgent;
  /** Hubs only: outer extent of the agent's cluster (drives the territory ring). */
  territoryRadius?: number;
};

export type OrbLayout = {
  nodes: LayoutNode[];
  links: OrbLink[];
};
