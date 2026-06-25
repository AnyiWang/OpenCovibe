import { describe, it, expect } from "vitest";
import { colorizeCommand } from "./shell-colorize";

/** Helper: strip HTML tags to get plain text */
function stripTags(html: string): string {
  return html.replace(/<[^>]*>/g, "");
}

/** Helper: check if a token is wrapped in a specific syntax class */
function hasColor(html: string, text: string, cls: string): boolean {
  return html.includes(`class="${cls}">${text}</span>`);
}

describe("colorizeCommand", () => {
  it("simple command: ls -la", () => {
    const html = colorizeCommand("ls -la");
    expect(hasColor(html, "$", "sh-prompt")).toBe(true); // prompt
    expect(hasColor(html, "ls", "sh-command")).toBe(true); // command name
    expect(hasColor(html, "-la", "sh-flag")).toBe(true); // flag
  });

  it("pipe: echo hello | grep h", () => {
    const html = colorizeCommand("echo hello | grep h");
    expect(hasColor(html, "|", "sh-operator")).toBe(true); // pipe
    expect(hasColor(html, "echo", "sh-command")).toBe(true); // first command
    expect(hasColor(html, "grep", "sh-command")).toBe(true); // command after pipe
  });

  it("quoted strings", () => {
    const html = colorizeCommand('echo "hello world"');
    expect(hasColor(html, "&quot;hello world&quot;", "sh-string")).toBe(true);
  });

  it("flags: npm install --save-dev", () => {
    const html = colorizeCommand("npm install --save-dev");
    expect(hasColor(html, "--save-dev", "sh-flag")).toBe(true);
  });

  it("redirection: cat file > out.txt", () => {
    const html = colorizeCommand("cat file > out.txt");
    expect(hasColor(html, "&gt;", "sh-operator")).toBe(true);
  });

  it("chained commands: cd /tmp && ls", () => {
    const html = colorizeCommand("cd /tmp && ls");
    expect(hasColor(html, "&amp;&amp;", "sh-operator")).toBe(true);
    expect(hasColor(html, "ls", "sh-command")).toBe(true); // command after &&
  });

  it("empty command: only $ prompt with trailing space", () => {
    const html = colorizeCommand("");
    expect(hasColor(html, "$", "sh-prompt")).toBe(true);
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
    expect(hasColor(html, "FOO=1", "sh-assign")).toBe(true); // assign
    expect(hasColor(html, "BAR=2", "sh-assign")).toBe(true); // assign
    expect(hasColor(html, "npm", "sh-command")).toBe(true); // command name
  });

  it("complex syntax graceful degradation: $(date)", () => {
    // Should not throw, unrecognized parts keep default color
    expect(() => colorizeCommand("echo $(date)")).not.toThrow();
    const html = colorizeCommand("echo $(date)");
    expect(html).toBeTruthy();
    expect(stripTags(html)).toContain("$(date)");
  });
});
