import {useCallback, useEffect, useState} from 'react';
import {Sidebar} from './components/Sidebar';
import {ChatView} from './components/ChatView';
import {WorkspacePage} from './components/WorkspacePage';
import {SkillPage} from './components/SkillPage';
import {ArtifactPage} from './components/ArtifactPage';
import {GroupPage} from './components/GroupPage';
import {AgentPage} from './components/AgentPage';
import {ProjectPage} from './components/ProjectPage';
import {Login} from './components/Login';
import {ErrorBanner} from './components/ErrorBanner';
import {api} from './api';
import {reportError, setErrorReporter, type ReportedError} from './errors';
import {useSessionKeepAlive} from './session';
import type {UserResponse} from './types';
import {isPageOf, type Page, type PageKind} from './page';

function App() {
  // undefined while the session is being checked, null when logged out.
  const [user, setUser] = useState<UserResponse | null | undefined>(undefined);
  const [errors, setErrors] = useState<ReportedError[]>([]);

  useEffect(() => {
    // At most a few at a time: older ones are dropped rather than piling up.
    setErrorReporter(error => setErrors(prev => [...prev.slice(-2), error]));
    return () => setErrorReporter(null);
  }, []);

  const dismissError = (id: number) => setErrors(prev => prev.filter(e => e.id !== id));

  // Keeps a session alive while the tab is visible, so that watching a
  // channel without sending anything does not end it (issue 31).
  useSessionKeepAlive(user?.token_lifetime_secs);

  useEffect(() => {
    api.setUnauthorizedHandler(() => setUser(null));
    api.me()
      .then(setUser)
      .catch((error) => {
        reportError('Failed to check the session', error);
        setUser(null);
      });
    return () => api.setUnauthorizedHandler(null);
  }, []);

  const handleLogout = async () => {
    try {
      await api.logout();
    } finally {
      setUser(null);
    }
  };

  let content;
  if (user === undefined) {
    content = <div className="flex h-screen w-full bg-white dark:bg-gray-900" />;
  } else if (user === null) {
    content = <Login onLogin={setUser} />;
  } else {
    // Keyed by user, so nothing of a previous user's view survives a re-login.
    content = <Layout key={user.id} user={user} onLogout={handleLogout} />;
  }
  return (
    <>
      <ErrorBanner errors={errors} onDismiss={dismissError} />
      {content}
    </>
  );
}

interface LayoutProps {
  user: UserResponse;
  onLogout: () => void;
}

/**
 * The sidebar, and beside it a channel or a page in its place - a workspace, a skill, an artifact,
 * a team, an agent or a project.
 */
function Layout({ user, onLogout }: LayoutProps) {
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [selectedChannelId, setSelectedChannelId] = useState<string | null>(null);
  // What is shown in the place of a channel: one of the user's workspaces
  // (issue 253), a skill (issue 22), an artifact (issue 291), a team
  // (issue 286), an agent (issue 227) or a project (issue 467).
  const [page, setPage] = useState<Page | null>(null);

  const handleSelectChannel = (sessionId: string, channelId: string) => {
    setPage(null);
    setSelectedSessionId(sessionId);
    setSelectedChannelId(channelId);
  };

  const handleSelectPage = (selected: Page) => {
    setSelectedChannelId(null);
    setPage(selected);
  };

  const handlePageGone = useCallback((kind: PageKind, id: string) => {
    setPage(current => (isPageOf(current, kind, id) ? null : current));
  }, []);

  const handleChannelDeleted = useCallback((channelId: string) => {
    setSelectedChannelId(current => (current === channelId ? null : current));
  }, []);

  const handleSessionDeleted = (sessionId: string) => {
    if (selectedSessionId === sessionId) {
      setSelectedSessionId(null);
      setSelectedChannelId(null);
    }
  };

  return (
    <div className="flex h-screen w-full overflow-hidden bg-white dark:bg-gray-900">
      <Sidebar
        user={user}
        onLogout={onLogout}
        selectedSessionId={selectedSessionId}
        selectedChannelId={selectedChannelId}
        onSelectChannel={handleSelectChannel}
        onSessionDeleted={handleSessionDeleted}
        onChannelDeleted={handleChannelDeleted}
        page={page}
        onSelectPage={handleSelectPage}
        onPageGone={handlePageGone}
      />
      
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {page?.kind === 'workspace' ? (
          <WorkspacePage key={page.id} workspaceId={page.id} userId={user.id} onDeleted={(id) => handlePageGone('workspace', id)} />
        ) : page?.kind === 'skill' ? (
          <SkillPage key={page.id} skillId={page.id} userId={user.id} onDeleted={(id) => handlePageGone('skill', id)} />
        ) : page?.kind === 'artifact' ? (
          <ArtifactPage key={page.id} artifactId={page.id} onDeleted={(id) => handlePageGone('artifact', id)} onSelectChannel={handleSelectChannel} onSelectPage={handleSelectPage} />
        ) : page?.kind === 'group' ? (
          <GroupPage key={page.id} groupId={page.id} userId={user.id} onDeleted={(id) => handlePageGone('group', id)} />
        ) : page?.kind === 'project' ? (
          <ProjectPage key={page.id} projectId={page.id} userId={user.id} onDeleted={(id) => handlePageGone('project', id)} onSelectPage={handleSelectPage} onSelectChannel={handleSelectChannel} />
        ) : page?.kind === 'agent' ? (
          <AgentPage key={page.id} agentId={page.id} userId={user.id} onDeleted={(id) => handlePageGone('agent', id)} onSelectChannel={handleSelectChannel} />
        ) : selectedSessionId && selectedChannelId ? (
          <ChatView
            sessionId={selectedSessionId}
            channelId={selectedChannelId}
            user={user}
            onSelectPage={handleSelectPage}
          />
        ) : (
          <div className="flex-1 flex items-center justify-center text-gray-500 dark:text-gray-400 p-4 text-center">
            <div>
              <h2 className="text-2xl font-semibold mb-2">Welcome to Caelumora</h2>
              <p>Select a channel from the sidebar to start chatting.</p>
            </div>
          </div>
        )}
      </main>
    </div>
  );
}

export default App;
