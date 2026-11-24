import { pageOwnerId } from '../pageOwner';

interface ChainLink {
  turn: Promise<void>;
  settled: Promise<void>;
  acceptedEpochs?: { base: number; after: number };
}

const sendChains = new Map<string, ChainLink>();

const lastInputSeq = new Map<string, number>();

export interface InputPosition {
  input_chain: string;
  input_seq: number;
}

function claimInputPosition(threadId: string): InputPosition {
  const input_seq = (lastInputSeq.get(threadId) ?? 0) + 1;
  lastInputSeq.set(threadId, input_seq);
  return { input_chain: pageOwnerId, input_seq };
}

export interface SendSlot {
  waitForTurn: Promise<void> | null;
  release: () => void;
  position: InputPosition;
  readonly intentEpoch?: number;
  epochForPost: () => number | undefined;
  accepted: (epoch: number | undefined) => void;
}

export function enterSendChain(threadId: string, intentEpoch?: number): SendSlot {
  const predecessor = sendChains.get(threadId);
  let release!: () => void;
  const settled = new Promise<void>((resolve) => {
    release = resolve;
  });
  const waitForTurn = predecessor
    ? predecessor.turn.then(() => predecessor.settled)
    : null;
  const link: ChainLink = { turn: waitForTurn ?? Promise.resolve(), settled };
  sendChains.set(threadId, link);
  void Promise.all([link.turn, settled]).then(() => {
    if (sendChains.get(threadId) === link) sendChains.delete(threadId);
  });
  let submittedEpoch: number | undefined;
  let causalBase = intentEpoch;
  let prepared = false;
  const epochForPost = (): number | undefined => {
    if (!prepared) {
      prepared = true;
      submittedEpoch = intentEpoch;
      const proof = predecessor?.acceptedEpochs;
      if (intentEpoch !== undefined && proof && intentEpoch >= proof.base && intentEpoch <= proof.after) {
        submittedEpoch = proof.after;
        causalBase = proof.base;
      }
    }
    return submittedEpoch;
  };
  return {
    waitForTurn, release, position: claimInputPosition(threadId), intentEpoch, epochForPost,
    accepted: (epoch) => {
      if (epoch === undefined) return;
      const before = epochForPost();
      if (before === undefined) {
        link.acceptedEpochs = { base: epoch - 1, after: epoch };
      } else if (causalBase !== undefined && epoch === before + 1) {
        link.acceptedEpochs = { base: causalBase, after: epoch };
      }
    },
  };
}
