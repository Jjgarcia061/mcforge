import { create } from "zustand";
import type { Setup } from "./api";

/** The saved setup, and whether the user is changing it in the wizard. */
interface Session {
  setup: Setup | null;
  editing: boolean;
}

export const useSession = create<Session>()(() => ({ setup: null, editing: false }));
