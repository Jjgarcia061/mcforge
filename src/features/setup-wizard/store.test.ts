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
    { id: "files", connectors: ["filesystem"], skills: {}, embedding: false },
    { id: "research", connectors: ["web", "filesystem"], skills: { analysis: 1 }, embedding: true },
    { id: "documents", connectors: [], skills: {}, embedding: true },
  ],
  models: { local: [], api: [], embedding: [] },
  focuses: [],
};

const state = () => useWizard.getState();
const at = (step: (typeof STEPS)[number]) => state().goTo(STEPS.indexOf(step));

describe("setup wizard", () => {
  beforeEach(() => useWizard.setState(initialState));

  it("tasks preselect the union of their connections and keep shared ones", () => {
    state().toggleUseCase(catalog, "files");
    expect(state().connectors).toEqual(["filesystem"]);
    state().toggleUseCase(catalog, "research");
    expect(state().connectors).toEqual(["filesystem", "web"]);
    state().toggleUseCase(catalog, "research");
    expect(state().connectors, "files still needs the filesystem").toEqual(["filesystem"]);
    expect(state().needs.useCases).toEqual(["files"]);
    state().toggleConnector("filesystem");
    expect(state().connectors).toEqual([]);
  });

  it("requires a task, then an engine", () => {
    at("useCase");
    expect(canContinue(state(), catalog)).toBe(false);
    state().toggleUseCase(catalog, "documents");
    expect(canContinue(state(), catalog), "a task with no connections is fine").toBe(true);

    at("engine");
    expect(canContinue(state(), catalog)).toBe(false);
    state().setEngine({ kind: "local", model: "qwen3:8b" });
    expect(canContinue(state(), catalog)).toBe(true);
  });

  it("blocks until required fields are filled", () => {
    state().toggleUseCase(catalog, "files");
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

  it("splits suggested connectors from the rest", () => {
    const ids = (cs: Connector[]) => cs.map((c) => c.id);
    const forFiles = suggestedConnectors(catalog, ["files"]);
    expect(ids(forFiles.suggested)).toEqual(["filesystem"]);
    expect(ids(forFiles.others)).toEqual(["web"]);
    const none = suggestedConnectors(catalog, ["documents"]);
    expect(none.suggested).toEqual([]);
    expect(ids(none.others)).toEqual(["filesystem", "web"]);
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
    state().toggleUseCase(catalog, "files");
    state().setLanguage("spanish");
    state().setPriority("quality");
    state().setValue("filesystem", "folders", ["/data"]);
    const setup = toSetup(state());
    expect(setup).toEqual({
      engine: { kind: "local", model: "qwen3:8b" },
      needs: { useCases: ["files"], language: "spanish", priority: "quality" },
      connectors: ["filesystem"],
      values: { filesystem: { folders: ["/data"] } },
      permissions: {},
    });
    const restored = fromSetup(setup!);
    expect(restored.step).toBe(STEPS.indexOf("engine"));
    expect(toSetup(restored)).toEqual(setup);
  });
});
