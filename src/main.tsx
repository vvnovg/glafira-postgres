import "./polyfills";
import "./i18n";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";
import { installPlainInputs } from "./lib/plainInputs";

installPlainInputs();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
