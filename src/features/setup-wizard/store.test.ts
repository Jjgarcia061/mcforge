import { beforeEach, describe, expect, it } from "vitest";
import type { Catalog, Connector } from "../../lib/api";
import {
  canContinue,
  fromSetup,
  initialState,
  missingFields,
  permissionOf,
  STEPS,
  suggestedConnectors,
  toSetup,
  useWizard,
} from "./store";

const files: Connector = {
  id: "filesystem",
  name: "Files",
  category: "files",
  description: "",
  homepage: null,
  verified: false,
  server: { runtime: "npx", package: "x", args: ["{{folders}}"], env: {} },
  fields: [{ key: "folders", label: "Folders", kind: "paths", required: true, placeholder: null, options: [] }],
  tools: [
    { name: "read", risk: "read", defaultPermission: "allow" },
    { name: "move", risk: "destructive", defaultPermission: "deny" },
  ],
  testable: false,
};

const web: Connector = { ...files, id: "web", fields: [], tools: [{ name: "fetch", risk: "network", defaultPermission: "ask" }] };

const catalog: Catalog = {
  connectors: [files, web],
  useCases: [
    { id: "files", connectors: ["filesystem"] },
    { id: "custom", connectors: [] },
  ],
  models: { local: [], api: [] },
  focuses: [],
};

const state = () => useWizard.getState();
const at = (step: (typeof STEPS)[number]) => state().goTo(STEPS.indexOf(step));

describe("setup wizard", () => {
  beforeEach(() => useWizard.setState(initialState));

  it("a use case preselects its connectors", () => {
    state().chooseUseCase(catalog, "files");
    expect(state().connectors).toEqual(["filesystem"]);
    state().toggleConnector("filesystem");
    expect(state().connectors).toEqual([]);
  });

  it("requires an engine and at least one connector", () => {
    at("engine");
    expect(canContinue(state(), catalog)).toBe(false);
    state().setEngine({ kind: "local", model: "qwen3:8b" });
    expect(canContinue(state(), catalog)).toBe(true);

    at("useCase");
    expect(canContinue(state(), catalog)).toBe(false);
  });

  it("blocks until required fields are filled", () => {
    state().chooseUseCase(catalog, "files");
    at("connections");
    expect(missingFields(state(), files)).toEqual(["folders"]);
    expect(canContinue(state(), catalog)).toBe(false);
    state().setValue("filesystem", "folders", ["/tmp/reports"]);
    expect(canContinue(state(), catalog)).toBe(true);
  });

  it("permissions default from risk and can be overridden", () => {
    expect(permissionOf(state(), files, "read")).toBe("allow");
    expect(permissionOf(state(), files, "move")).toBe("deny");
    state().setPermission("filesystem", "move", "ask");
    expect(permissionOf(state(), files, "move")).toBe("ask");
  });

  it("splits suggested connectors from the rest; custom suggests all", () => {
    const ids = (cs: Connector[]) => cs.map((c) => c.id);
    const forFiles = suggestedConnectors(catalog, "files");
    expect(ids(forFiles.suggested)).toEqual(["filesystem"]);
    expect(ids(forFiles.others)).toEqual(["web"]);
    const custom = suggestedConnectors(catalog, "custom");
    expect(ids(custom.suggested)).toEqual(["filesystem", "web"]);
    expect(custom.others).toEqual([]);
  });

  it("never navigates outside the steps", () => {
    state().goTo(99);
    expect(state().step).toBe(STEPS.length - 1);
    state().goTo(-1);
    expect(state().step).toBe(0);
  });

  it("round-trips the answers through a saved setup", () => {
    expect(toSetup(state())).toBeNull();
    state().setEngine({ kind: "local", model: "qwen3:8b" });
    state().chooseUseCase(catalog, "files");
    state().setValue("filesystem", "folders", ["/data"]);
    const setup = toSetup(state());
    expect(setup).toEqual({
      engine: { kind: "local", model: "qwen3:8b" },
      useCase: "files",
      connectors: ["filesystem"],
      values: { filesystem: { folders: ["/data"] } },
      permissions: {},
    });
    const restored = fromSetup(setup!);
    expect(restored.step).toBe(STEPS.indexOf("engine"));
    expect(toSetup(restored)).toEqual(setup);
  });
});
