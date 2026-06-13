import { useState } from 'react';
import { Sidebar } from './components/Sidebar';
import { ChatView } from './components/ChatView';

function App() {
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [selectedChannelId, setSelectedChannelId] = useState<string | null>(null);

  const handleSelectChannel = (sessionId: string, channelId: string) => {
    setSelectedSessionId(sessionId);
    setSelectedChannelId(channelId);
  };

  return (
    <div className="flex h-screen w-full overflow-hidden bg-white dark:bg-gray-900">
      <Sidebar
        selectedSessionId={selectedSessionId}
        selectedChannelId={selectedChannelId}
        onSelectChannel={handleSelectChannel}
      />
      
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {selectedSessionId && selectedChannelId ? (
          <ChatView
            sessionId={selectedSessionId}
            channelId={selectedChannelId}
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
