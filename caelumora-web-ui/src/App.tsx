import {useCallback, useEffect, useState} from 'react';
import {Sidebar} from './components/Sidebar';
import {ChatView} from './components/ChatView';
import {WorkspacePage} from './components/WorkspacePage';
import {Login} from './components/Login';
import {ErrorBanner} from './components/ErrorBanner';
import {api} from './api';
import {reportError, setErrorReporter, type ReportedError} from './errors';
import {useSessionKeepAlive} from './session';
import type {UserResponse} from './types';

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

/** The sidebar, and beside it a channel or one of the user's workspaces. */
function Layout({ user, onLogout }: LayoutProps) {
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [selectedChannelId, setSelectedChannelId] = useState<string | null>(null);
  // One of the user's workspaces (issue 253), shown in the place of a channel.
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string | null>(null);

  const handleSelectChannel = (sessionId: string, channelId: string) => {
    setSelectedWorkspaceId(null);
    setSelectedSessionId(sessionId);
    setSelectedChannelId(channelId);
  };

  const handleSelectWorkspace = (workspaceId: string) => {
    setSelectedChannelId(null);
    setSelectedWorkspaceId(workspaceId);
  };

  const handleWorkspaceDeleted = useCallback((workspaceId: string) => {
    setSelectedWorkspaceId(current => (current === workspaceId ? null : current));
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
        selectedWorkspaceId={selectedWorkspaceId}
        onSelectWorkspace={handleSelectWorkspace}
        onWorkspaceDeleted={handleWorkspaceDeleted}
      />
      
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {selectedWorkspaceId ? (
          <WorkspacePage key={selectedWorkspaceId} workspaceId={selectedWorkspaceId} onDeleted={handleWorkspaceDeleted} />
        ) : selectedSessionId && selectedChannelId ? (
          <ChatView
            sessionId={selectedSessionId}
            channelId={selectedChannelId}
            user={user}
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
