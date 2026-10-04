import React from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import {
  getTheme,
  getBackgroundImage,
  getBackgroundOpacity,
  applyBackground,
  MIKU_BG,
} from "./Settings";
import "./styles.css";

document.documentElement.dataset.theme = getTheme();
const startupTheme = document.documentElement.dataset.theme;
applyBackground(
  getBackgroundImage() || (startupTheme === "miku" ? MIKU_BG : null),
  getBackgroundOpacity(),
);
createRoot(document.getElementById("root")).render(<App />);
