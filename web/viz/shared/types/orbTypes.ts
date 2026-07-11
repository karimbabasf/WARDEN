// orbTypes.ts: the shared layout primitives the RADAR constellation lays out and
// renders. `layoutRadarScene` produces an `OrbLayout` of `LayoutNode`s (one per live
// agent) that the scene, camera framing, and detail panel all read.

export type Vec3 = { x: number; y: number; z: number };

export type LayoutNodeKind = 'hub' | 'issue';

export type LayoutNode = {
  id: string;
  kind: LayoutNodeKind;
  position: Vec3;
  radius: number;
  agentId: string;
  harness: string;
  /**
   * The live agent this node represents, set by `layoutRadarScene`.
   */
  radarAgent?: import('./radarTypes').RadarAgent;
  /** The agent's depth in the forest (0 = root planet). */
  depth?: number;
};

export type OrbLink = {
  source: string;
  target: string;
  kind: 'agent_issue';
};

export type OrbLayout = {
  nodes: LayoutNode[];
  links: OrbLink[];
};
