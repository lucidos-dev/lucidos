import { useRef, useLayoutEffect } from 'preact/hooks';
import { focusedThreadId, activeExchanges, promptAnimating, activeThreadIsComposing } from '../../store/store';
import { PromptInput } from '../chat/PromptInput';
import { CreateThreadView } from '../chat/CreateThreadView';
import { ThreadView } from '../chat/ThreadView';
import { ThreadToggleButton } from '../shared/ThreadToggleButton';
import { prefersReducedMotion } from '../../utils/platform';
import { isMobile } from '../../utils/viewport';

export function ThreadPane() {
  const tid = focusedThreadId.value;
  const isEmpty = activeExchanges.value.length === 0;
  // Composing drafts share the brand-new compose layout — prompt re-docks only after Send.
  const isComposingDraft = activeThreadIsComposing.value;
  const isComposeEmpty = (!tid && isEmpty) || isComposingDraft;

  const promptRef = useRef<HTMLDivElement>(null);
  const promptYRef = useRef<number | null>(null);
  const prevComposeEmpty = useRef(isComposeEmpty);

  // Capture prompt position before render for FLIP animation —
  // only when isComposeEmpty is changing (avoid getBoundingClientRect on every render)
  if (promptRef.current && prevComposeEmpty.current !== isComposeEmpty) {
    promptYRef.current = promptRef.current.getBoundingClientRect().top;
  }

  // FLIP animation: smoothly slide prompt between centered and bottom positions
  useLayoutEffect(() => {
    if (prevComposeEmpty.current === isComposeEmpty) return;
    prevComposeEmpty.current = isComposeEmpty;

    const el = promptRef.current;
    if (!el || promptYRef.current === null) return;

    if (prefersReducedMotion()) return;
    // Skip on mobile — CSS transforms on the prompt area prevent iOS Safari
    // from opening the keyboard when the textarea is programmatically focused.
    if (isMobile()) return;

    const newY = el.getBoundingClientRect().top;
    const delta = promptYRef.current - newY;

    if (Math.abs(delta) < 5) return;

    // Gate content while prompt animates — compose→thread gates ThreadView
    promptAnimating.value = true;

    el.style.transform = `translateY(${delta}px)`;
    el.style.transition = 'none';

    const clearAnimation = () => {
      clearTimeout(safetyTimer);
      el.removeEventListener('transitionend', onTransitionEnd);
      promptAnimating.value = false;
    };

    const onTransitionEnd = (e: TransitionEvent) => {
      if (e.target !== el || e.propertyName !== 'transform') return;
      clearAnimation();
    };

    // Safety timeout: if transitionend doesn't fire (e.g. element off-screen
    // on mobile during pane swipe), clear the gate so content isn't hidden.
    let safetyTimer = setTimeout(clearAnimation, 400);

    let raf1: number | undefined;
    let raf2: number | undefined;
    raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => {
        el.addEventListener('transitionend', onTransitionEnd);
        el.style.transition = 'transform 0.3s ease';
        el.style.transform = '';
      });
    });

    return () => {
      if (raf1) cancelAnimationFrame(raf1);
      if (raf2) cancelAnimationFrame(raf2);
      clearAnimation();
    };
  }, [isComposeEmpty]);

  return (
    <div class={`thread-pane${isComposeEmpty ? ' compose-empty' : ''}`} data-drop-zone="attach">
      <ThreadToggleButton class="thread-pane-toggle" />
      <div class="thread-pane-body">
        {tid && !isComposingDraft ? <div class="thread-view-clip"><ThreadView /></div> : <CreateThreadView />}
        <div class="prompt-area" ref={promptRef}>
          <PromptInput />
        </div>
      </div>
    </div>
  );
}
