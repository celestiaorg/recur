/// <reference types="svelte" />
/// <reference types="vite/client" />

interface Navigator {
  userAgentData?: {
    getHighEntropyValues(hints: string[]): Promise<{ architecture?: string }>;
  };
}
