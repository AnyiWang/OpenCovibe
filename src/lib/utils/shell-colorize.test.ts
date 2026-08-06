import { describe, it, expect } from "vitest";
import { colorizeCommand } from "./shell-colorize";

/** Helper: strip HTML tags to get plain text */
function stripTags(html: string): string {
  return html.replace(/<[^>]*>/g, "");
}

/** Helper: check if a token has a specific color */
function hasColor(html: string, text: string, color: string): boolean {
  return html.includes(`color:${color}">${text}</span>`);
}

describe("colorizeCommand", () => {
  it("simple command: ls -la", () => {
    const html = colorizeCommand("ls -la");
    expect(hasColor(html, "$", "#4ade80")).toBe(true); // prompt green
    expect(hasColor(html, "ls", "#e5e7eb")).toBe(true); // command white
    expect(hasColor(html, "-la", "#22d3ee")).toBe(true); // flag cyan
  });

  it("pipe: echo hello | grep h", () => {
    const html = colorizeCommand("echo hello | grep h");
    expect(hasColor(html, "|", "#c084fc")).toBe(true); // pipe magenta
    expect(hasColor(html, "echo", "#e5e7eb")).toBe(true); // first command
    expect(hasColor(html, "grep", "#e5e7eb")).toBe(true); // command after pipe
  });

  it("quoted strings", () => {
    const html = colorizeCommand('echo "hello world"');
    expect(hasColor(html, "&quot;hello world&quot;", "#facc15")).toBe(true); // yellow
  });

  it("flags: npm install --save-dev", () => {
    const html = colorizeCommand("npm install --save-dev");
    expect(hasColor(html, "--save-dev", "#22d3ee")).toBe(true); // cyan
  });

  it("redirection: cat file > out.txt", () => {
    const html = colorizeCommand("cat file > out.txt");
    expect(hasColor(html, "&gt;", "#c084fc")).toBe(true); // magenta
  });

  it("chained commands: cd /tmp && ls", () => {
    const html = colorizeCommand("cd /tmp && ls");
    expect(hasColor(html, "&amp;&amp;", "#c084fc")).toBe(true); // magenta
    expect(hasColor(html, "ls", "#e5e7eb")).toBe(true); // command after &&
  });

  it("empty command: only $ prompt with trailing space", () => {
    const html = colorizeCommand("");
    expect(hasColor(html, "$", "#4ade80")).toBe(true);
    expect(stripTags(html)).toBe("$ ");
  });

  it("XSS protection: <script> is escaped", () => {
    const html = colorizeCommand('echo "<script>alert(1)</script>"');
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });

  it("preserves whitespace", () => {
    const html = colorizeCommand('echo  "a   b"');
    // Double space between echo and "a   b"
    expect(stripTags(html).includes("  ")).toBe(true);
    // Inner spaces in quoted string preserved
    expect(html).toContain("a   b");
  });

  it("env var assignment: FOO=1 BAR=2 npm run dev", () => {
    const html = colorizeCommand("FOO=1 BAR=2 npm run dev");
    expect(hasColor(html, "FOO=1", "#22d3ee")).toBe(true); // assign cyan
    expect(hasColor(html, "BAR=2", "#22d3ee")).toBe(true); // assign cyan
    expect(hasColor(html, "npm", "#e5e7eb")).toBe(true); // command white
  });

  it("complex syntax graceful degradation: $(date)", () => {
    // Should not throw, unrecognized parts keep default color
    expect(() => colorizeCommand("echo $(date)")).not.toThrow();
    const html = colorizeCommand("echo $(date)");
    expect(html).toBeTruthy();
    expect(stripTags(html)).toContain("$(date)");
  });
});
