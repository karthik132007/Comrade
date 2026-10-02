import React from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { getTheme } from "./Settings";
import "./styles.css";

document.documentElement.dataset.theme = getTheme();
createRoot(document.getElementById("root")).render(<App />);
