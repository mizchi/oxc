import { describe, expect, it } from "vitest";

import { transform, transformSync } from "../index";

const fixture = "const data = graphql`query FooQuery { id }`;\n";

describe("transformSync", () => {
  it("hoists an ES import by default", () => {
    const result = transformSync("foo.js", fixture);

    expect(result.errors).toEqual([]);
    expect(result.code).toMatchInlineSnapshot(`
      "import _FooQuery from "./__generated__/FooQuery.graphql.js";
      const data = _FooQuery;
      "
    `);
  });

  it("emits require() calls when eagerEsModules is disabled", () => {
    const result = transformSync("foo.js", fixture, { eagerEsModules: false });

    expect(result.errors).toEqual([]);
    expect(result.code).toMatchInlineSnapshot(`
      "const data = require("./__generated__/FooQuery.graphql.js");
      "
    `);
  });

  it("resolves artifactDirectory relative to the file", () => {
    const result = transformSync("project/src/pages/foo.js", fixture, {
      artifactDirectory: "project/src/__generated__",
      eagerEsModules: false,
    });

    expect(result.errors).toEqual([]);
    expect(result.code).toContain('require("../__generated__/FooQuery.graphql.js")');
  });

  it("uses the .ts artifact extension for the typescript language", () => {
    const result = transformSync("foo.js", fixture, {
      language: "typescript",
      eagerEsModules: false,
    });

    expect(result.errors).toEqual([]);
    expect(result.code).toContain('require("./__generated__/FooQuery.graphql.ts")');
  });

  it("preserves TypeScript and JSX syntax", () => {
    const result = transformSync(
      "Foo.tsx",
      `interface Props { id: string }
const data = graphql\`query FooQuery { id }\`;
export const App = (props: Props) => <div>{data}</div>;
`,
    );

    expect(result.errors).toEqual([]);
    expect(result.code).toContain("interface Props");
    expect(result.code).toContain("<div>{data}</div>");
    expect(result.code).toContain('import _FooQuery from "./__generated__/FooQuery.graphql.js"');
  });

  it("errors on unnamed GraphQL definitions", () => {
    const result = transformSync("foo.js", "const data = graphql`{ id }`;");

    expect(result.code).toBe("");
    expect(result.errors.length).toBeGreaterThan(0);
    expect(result.errors[0].message).toContain("named GraphQL");
  });

  it("errors on invalid options", () => {
    const result = transformSync("foo.js", fixture, { language: "elm" });

    expect(result.code).toBe("");
    expect(result.errors.length).toBeGreaterThan(0);
    expect(result.errors[0].message).toContain("language");
  });

  it("leaves non-graphql tags untouched", () => {
    const source = "const style = css`color: red;`;\n";
    const result = transformSync("foo.js", source);

    expect(result.errors).toEqual([]);
    expect(result.code).toBe(source);
  });

  it("generates a source map when requested", () => {
    const result = transformSync("foo.js", fixture, { sourcemap: true });

    expect(result.errors).toEqual([]);
    expect(result.map).toBeDefined();
    expect(result.map?.sources).toContain("foo.js");
  });
});

describe("transform", () => {
  it("transforms asynchronously", async () => {
    const result = await transform("foo.js", fixture, { eagerEsModules: false });

    expect(result.errors).toEqual([]);
    expect(result.code).toContain('require("./__generated__/FooQuery.graphql.js")');
  });
});
