import { Component, type ReactNode } from "react";

/** Shows `fallback` where `children` threw while drawing, so the rest of the page keeps drawing; a new `rev` tries them again. */
export class Boundary extends Component<{ name: string; rev: unknown; fallback: ReactNode; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: unknown) {
    console.error(`${this.props.name} failed to draw`, error);
  }

  componentDidUpdate(previous: { rev: unknown }) {
    if (previous.rev !== this.props.rev && this.state.failed) this.setState({ failed: false });
  }

  render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}
