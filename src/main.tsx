import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

async function boot() {
  // Dev-only: run the UI in a plain browser against a fake backend (see src/devMock.ts).
  const params = new URLSearchParams(window.location.search);
  if (import.meta.env.DEV && params.has("mock")) {
    (await import("./devMock")).installMock(params.get("mock") ?? "");
  }
  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

boot();
