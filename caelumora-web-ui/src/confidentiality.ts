// What a channel's classification means for the user (issue 133): how
// sensitive its contents are, and for whom - its compartments.

/** The levels of a classification, from the most open. */
export type ConfidentialityLevel = 'public' | 'internal' | 'confidential' | 'strictly_confidential';

/**
 * A classification: a level and an audience of compartments. Without
 * compartments it is general; `public` takes none.
 */
export interface Classification {
  level: ConfidentialityLevel;
  /** Absent means none. */
  compartments?: string[];
}

interface ConfidentialityInfo {
  label: string;
  description: string;
}

const LEVELS: Record<ConfidentialityLevel, ConfidentialityInfo> = {
  public: {label: 'Public', description: 'Anyone may read what is said here.'},
  internal: {label: 'Internal', description: 'For the organisation: channels of the same or a higher level may read it.'},
  confidential: {label: 'Confidential', description: 'Only confidential and strictly confidential channels may read it.'},
  strictly_confidential: {label: 'Strictly confidential', description: 'Only strictly confidential channels may read it.'},
};

/** The levels, from the most open, for choosing one. */
export const CONFIDENTIALITY_LEVELS = Object.keys(LEVELS) as ConfidentialityLevel[];

/** The classification of a channel whose creator names none - the server's default. */
export const DEFAULT_CLASSIFICATION: Classification = {level: 'internal'};

/** The label and explanation of a level; one this UI does not know is shown by its name. */
export function confidentialityInfo(level: string): ConfidentialityInfo {
  return LEVELS[level as ConfidentialityLevel] ?? {label: level, description: `Confidentiality "${level}".`};
}

/** A classification for display, e.g. `Internal · hr, finance`. */
export function classificationLabel(classification: Classification): string {
  const level = confidentialityInfo(classification.level).label;
  const compartments = classification.compartments ?? [];
  return compartments.length === 0 ? level : `${level} · ${compartments.join(', ')}`;
}

/** What a classification means, for a tooltip. */
export function classificationDescription(classification: Classification): string {
  const level = confidentialityInfo(classification.level).description;
  const compartments = classification.compartments ?? [];
  return compartments.length === 0
    ? `${level} No compartment: channels with a compartment may read it too.`
    : `${level} Meant for ${compartments.join(', ')}: only channels whose every compartment may read one of these.`;
}
