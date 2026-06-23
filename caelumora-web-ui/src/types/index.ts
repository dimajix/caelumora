export interface SessionId {
  0: string;
}

export interface ChannelId {
  0: string;
}

export interface SessionResponse {
  id: string;
  name: string;
}

export interface ChannelResponse {
  id: string;
  name: string;
  participants: string[];
}

export interface Message {
  id?: string;
  role: 'User' | 'Assistant' | 'System';
  content: string | null;
  sender_name?: string;
  timestamp?: string;
  [key: string]: any;
}

export interface ActorEvent {
  ResponseStarted?: any;
  PartialMessageGenerated?: Message;
  CompleteMessageGenerated?: Message;
  ToolExecutionRequested?: any;
  ToolExecutionGranted?: any;
  ToolExecutionDenied?: any;
  ResponseRequested?: any;
  FullResponseGenerated?: Message[];
}

export interface Envelope {
  timestamp: string;
  event: ActorEvent;
  context: any;
  security: any;
}

export interface SessionTemplate {
  id: string;
  metadata: {
    name: string;
    description?: string;
  };
}

export interface CreateSessionRequest {
  template_id: string;
  name: string;
}
