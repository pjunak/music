import { useState } from "react";
import type { FormEvent } from "react";

import { ApiError } from "@/core/api";
import { useAuthStore } from "@/core/auth";

export default function LoginPage() {
  const login = useAuthStore((s) => s.login);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  async function onSubmit(event: FormEvent) {
    event.preventDefault();
    setError(null);
    setPending(true);
    try {
      await login(username, password);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        setError("Invalid credentials.");
      } else {
        setError("Login failed. Check the server is running.");
      }
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="centered">
      <form className="login" onSubmit={onSubmit}>
        <h1>Music</h1>
        <label>
          Username
          <input
            autoFocus
            autoComplete="username"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            required
          />
        </label>
        <label>
          Password
          <input
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
          />
        </label>
        {error ? <p className="error">{error}</p> : null}
        <button type="submit" disabled={pending}>
          {pending ? "Signing in…" : "Sign in"}
        </button>
      </form>
    </div>
  );
}
