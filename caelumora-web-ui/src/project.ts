// How a project (issue 467) is called and loaded: a shared one by its name, a session's own - named
// `own` alike for every session - by its session.

import {useEffect, useState} from 'react';
import type {ProjectResponse} from './types';
import {api} from './api';

/** What `project` is called, its session named `sessionName` where it is a session's own. */
export function projectLabel(project: ProjectResponse, sessionName: string | null): string {
  if (project.scope !== 'session') return project.name;
  return sessionName ? `Own project of ${sessionName}` : 'Own project of its session';
}

/** What `project` is called, the name of its session fetched for a session's own. */
export async function labelOf(project: ProjectResponse): Promise<string> {
  const sessionName = project.session_id ? (await api.getSession(project.session_id).catch(() => null))?.name ?? null : null;
  return projectLabel(project, sessionName);
}

/**
 * Where `project` lies, where that is worth saying: `global`. One of a user's scope is not called
 * "yours": the API names no owner, and a project one works on may be another's.
 */
export function placeOf(project: ProjectResponse): string | null {
  return project.scope === 'global' ? 'global' : null;
}

/** `project` as an option of a list: its name, and where it lies where that is worth saying. */
export function optionLabel(project: ProjectResponse): string {
  const place = placeOf(project);
  return place ? `${project.name} (${place})` : project.name;
}

/**
 * Project `projectId` as the user reads it, with what it is called; `null` while it is loaded, and
 * for one the user may not read.
 */
export function useProject(projectId: string | undefined): {project: ProjectResponse; label: string} | null {
  const [found, setFound] = useState<{project: ProjectResponse; label: string} | null>(null);
  useEffect(() => {
    if (!projectId) return;
    let current = true;
    const load = async () => {
      const project = await api.getProject(projectId);
      const label = await labelOf(project);
      if (current) setFound({project, label});
    };
    // Who may not read it sees no project.
    load().catch(() => { if (current) setFound(null); });
    return () => { current = false; };
  }, [projectId]);
  return found;
}
