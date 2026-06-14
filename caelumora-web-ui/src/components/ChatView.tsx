import React, { useState, useEffect, useRef } from 'react';
import { Send, Users, Square } from 'lucide-react';
import type { Message, Envelope, ChannelResponse } from '../types';
import { api } from '../api';
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

interface ChatViewProps {
  sessionId: string;
  channelId: string;
}

export const ChatView: React.FC<ChatViewProps> = ({ sessionId, channelId }) => {
  const [messages, setMessages] = useState<Message[]>([]);
  const [channel, setChannel] = useState<ChannelResponse | null>(null);
  const [inputText, setInputText] = useState('');
  const [isSending, setIsSending] = useState(false);
  const [isGenerating, setIsGenerating] = useState(false);
  const [streamingMessage, setStreamingMessage] = useState<string | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    loadMessages();
    loadChannel();
    const eventSource = api.getEventSource(sessionId, channelId);

    eventSource.onmessage = (event) => {
      try {
        const envelope: Envelope = JSON.parse(event.data);
        const actorEvent = envelope.event;
        if (actorEvent.PartialMessageGenerated) {
          setIsGenerating(true);
          setStreamingMessage(prev => (prev || '') + (actorEvent.PartialMessageGenerated!.content || ''));
        } else if (actorEvent.CompleteMessageGenerated) {
          //setMessages(prev => [...prev, actorEvent.CompleteMessageGenerated!]);
          setStreamingMessage(null);
        } else if (actorEvent.FullResponseGenerated) {
          setMessages(prev => [...prev, ...actorEvent.FullResponseGenerated!]);
          setStreamingMessage(null);
          setIsGenerating(false);
        }
      } catch (e) {
        console.error('Failed to parse SSE event', e);
      }
    };

    eventSource.onerror = (e) => {
      console.error('SSE Error', e);
      eventSource.close();
    };

    return () => {
      eventSource.close();
    };
  }, [sessionId, channelId]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, streamingMessage]);

  const loadMessages = async () => {
    try {
      const data = await api.getMessages(sessionId, channelId);
      setMessages(data);
      setStreamingMessage(null);
      setIsGenerating(false);
    } catch (error) {
      console.error('Failed to load messages', error);
    }
  };

  const loadChannel = async () => {
    try {
      const data = await api.getChannel(sessionId, channelId);
      setChannel(data);
    } catch (error) {
      console.error('Failed to load channel details', error);
    }
  };

  const handleSendMessage = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!inputText.trim() || isSending) return;

    const text = inputText;
    setInputText('');
    setIsSending(true);

    // Optimistic update
    //const userMsg: Message = { role: 'User', content: text };
    //setMessages(prev => [...prev, userMsg]);

    try {
      await api.sendMessage(sessionId, channelId, text);
    } catch (error) {
      console.error('Failed to send message', error);
      // Remove optimistic message on error or show error state
    } finally {
      setIsSending(false);
    }
  };

  const handleStopGeneration = async () => {
    try {
      await api.cancelChannel(sessionId, channelId);
    } catch (error) {
      console.error('Failed to cancel generation', error);
    }
  };

  return (
    <div className="flex-1 flex overflow-hidden bg-gray-50 dark:bg-gray-800">
      <div className="flex-1 flex flex-col min-w-0">
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {messages.map((msg, idx) => (
            <div
              key={idx}
              className={cn(
                "max-w-[80%] p-3 rounded-lg shadow-sm",
                msg.role === 'User' 
                  ? "ml-auto bg-blue-600 text-white rounded-br-none" 
                  : "bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600"
              )}
            >
              <div className="text-xs font-semibold mb-1 opacity-70 uppercase tracking-wider">
                {msg.sender_name || msg.role}
              </div>
              <div className="whitespace-pre-wrap">{msg.content}</div>
            </div>
          ))}
          {streamingMessage && (
            <div className="max-w-[80%] p-3 rounded-lg shadow-sm bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600">
              <div className="text-xs font-semibold mb-1 opacity-70 uppercase tracking-wider">
                {isGenerating ? "Generating..." : "Stopped"}
              </div>
              <div className="whitespace-pre-wrap">{streamingMessage}</div>
            </div>
          )}
          <div ref={messagesEndRef} />
        </div>

        <div className="p-4 bg-white dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700">
          <form onSubmit={handleSendMessage} className="flex space-x-2">
            <input
              type="text"
              value={inputText}
              onChange={(e) => setInputText(e.target.value)}
              placeholder="Type your message..."
              disabled={isSending}
              className="flex-1 p-2 border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <button
              type="submit"
              disabled={isSending || isGenerating || !inputText.trim()}
              className="p-2 bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white rounded-md transition-colors"
            >
              <Send size={20} />
            </button>
            {isGenerating && (
              <button
                type="button"
                onClick={handleStopGeneration}
                className="p-2 bg-red-600 hover:bg-red-700 text-white rounded-md transition-colors"
                title="Stop generation"
              >
                <Square size={20} fill="currentColor" />
              </button>
            )}
          </form>
        </div>
      </div>

      <div className="w-64 border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 overflow-y-auto hidden md:block">
        <div className="p-4">
          <div className="flex items-center space-x-2 mb-4 text-gray-500 dark:text-gray-400">
            <Users size={18} />
            <h3 className="font-semibold uppercase text-xs tracking-wider">Channel Members</h3>
          </div>
          <div className="space-y-2">
            {channel?.participants.map((name, idx) => (
              <div key={idx} className="flex items-center space-x-2 p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors">
                <div className="w-2 h-2 rounded-full bg-green-500" />
                <span className="text-sm text-gray-700 dark:text-gray-300">{name}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
};
