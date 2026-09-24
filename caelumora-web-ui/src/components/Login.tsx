import React, {useState} from 'react';
import {LogIn} from 'lucide-react';
import type {UserResponse} from '../types';
import {api} from '../api';

interface LoginProps {
  onLogin: (user: UserResponse) => void;
}

export const Login: React.FC<LoginProps> = ({ onLogin }) => {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!username.trim() || isSubmitting) return;

    setIsSubmitting(true);
    setError(null);
    try {
      onLogin(await api.login(username.trim(), password));
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Login failed');
      setPassword('');
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <div className="flex h-screen w-full items-center justify-center bg-gray-50 dark:bg-gray-800 p-4">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-sm bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-lg shadow-xs p-6 space-y-4"
      >
        <h1 className="text-xl font-bold text-gray-900 dark:text-gray-100">Caelumora</h1>
        <p className="text-sm text-gray-500 dark:text-gray-400">Sign in to continue.</p>

        <label className="block space-y-1">
          <span className="text-sm text-gray-700 dark:text-gray-300">Username</span>
          <input
            type="text"
            autoComplete="username"
            autoFocus
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            disabled={isSubmitting}
            className="w-full p-2 border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-hidden focus:ring-2 focus:ring-blue-500"
          />
        </label>

        <label className="block space-y-1">
          <span className="text-sm text-gray-700 dark:text-gray-300">Password</span>
          <input
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            disabled={isSubmitting}
            className="w-full p-2 border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-hidden focus:ring-2 focus:ring-blue-500"
          />
        </label>

        {error && (
          <div role="alert" className="text-sm text-red-600 dark:text-red-400">{error}</div>
        )}

        <button
          type="submit"
          disabled={isSubmitting || !username.trim()}
          className="w-full flex items-center justify-center gap-2 p-2 bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white rounded-md transition-colors"
        >
          <LogIn size={18} />
          {isSubmitting ? 'Signing in…' : 'Sign in'}
        </button>
      </form>
    </div>
  );
};
