import { useEffect } from "react";
import { Navigate, Route, Routes } from "react-router-dom";

import { useAuthStore } from "@/core/auth";
import AppShell from "@/shell/AppShell";
import LoginPage from "@/shell/LoginPage";

export default function App() {
  const { status, refresh } = useAuthStore();

  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (status === "unknown") {
    return <div className="centered">Loading…</div>;
  }

  return (
    <Routes>
      <Route
        path="/login"
        element={status === "authenticated" ? <Navigate to="/" replace /> : <LoginPage />}
      />
      <Route
        path="/*"
        element={status === "authenticated" ? <AppShell /> : <Navigate to="/login" replace />}
      />
    </Routes>
  );
}
