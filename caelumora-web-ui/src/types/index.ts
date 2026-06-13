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
}

export interface Message {
  role: 'User' | 'Assistant' | 'System';
  content: string | null;
  [key: string]: any;
}

export interface ActorEvent {
  PartialMessageGenerated?: {
    role: string;
    content: string;
  };
  CompleteMessageGenerated?: Message;
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
