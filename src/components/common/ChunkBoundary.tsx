import { Component, type ReactNode } from "react";

/** Сбой ленивого чанка (справка, заметки к обновлению) или его рендера показывает `fallback`, не роняя окно. */
export class ChunkBoundary extends Component<{ fallback: ReactNode; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}
