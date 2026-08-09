import { useRef, useLayoutEffect } from 'preact/hooks';
import { focusedThreadId, promptAnimating, promptSendCollapsing, activeThreadIsComposing, composeViewActive } from '../../store/store';
import { PromptInput } from '../chat/PromptInput';
import { CreateThreadView } from '../chat/CreateThreadView';
import { ThreadView } from '../chat/ThreadView';
import { prefersReducedMotion } from '../../utils/platform';
import { isMobile } from '../../utils/viewport';

export function ThreadPane() {
  const tid = focusedThreadId.value;
  // Composing drafts share the brand-new compose layout — prompt re-docks only after Send.
  const isComposingDraft = activeThreadIsComposing.value;
  // Shared with PromptInput's height animation via composeViewActive so both agree.
  const isComposeEmpty = composeViewActive.value;

  const promptRef = useRef<HTMLDivElement>(null);
  const promptYRef = useRef<number | null>(null);
  const promptTaHeightRef = useRef<number | null>(null);
  const prevComposeEmpty = useRef(isComposeEmpty);

  // Capture prompt position (and textarea height) before render for the FLIP —
  // only when isComposeEmpty is changing (avoid getBoundingClientRect on every
  // render). The height is read while the textarea is still tall (submit()
  // deferred its reset), so the collapse can animate from the real start height.
  if (promptRef.current && prevComposeEmpty.current !== isComposeEmpty) {
    promptYRef.current = promptRef.current.getBoundingClientRect().top;
    const ta = promptRef.current.querySelector<HTMLElement>('.prompt-textarea');
    promptTaHeightRef.current = ta ? ta.offsetHeight : null;
  }

  // FLIP animation: smoothly slide the prompt between the centered compose
  // position and the docked follow-up position — and, on send, shrink the
  // textarea height in lockstep so a tall draft eases down to its docked state
  // instead of snapping short first.
  useLayoutEffect(() => {
    if (prevComposeEmpty.current === isComposeEmpty) return;
    const leavingCompose = !isComposeEmpty;
    prevComposeEmpty.current = isComposeEmpty;

    // Consume the send-collapse ticket set by PromptInput.submit(). It deferred
    // the textarea height reset so we can animate the collapse; we own that
    // reset here in EVERY exit path below, so it can never stick tall.
    const collapsing = promptSendCollapsing.peek();
    if (collapsing) promptSendCollapsing.value = false;
    const shouldCollapse = collapsing && leavingCompose;

    const el = promptRef.current;
    const ta = el?.querySelector<HTMLElement>('.prompt-textarea') ?? null;
    // Drop the deferred inline height → CSS min-height defines the docked target.
    const resetTaHeight = () => { if (ta) { ta.style.transition = ''; ta.style.height = ''; } };

    if (!el || promptYRef.current === null) {
      if (shouldCollapse) resetTaHeight();
      return;
    }

    if (prefersReducedMotion() || isMobile()) {
      // Skip the slide — CSS transforms on the prompt area prevent iOS Safari
      // from opening the keyboard on programmatic focus — but still honor the
      // deferred collapse so the textarea doesn't stay tall.
      if (shouldCollapse) resetTaHeight();
      return;
    }

    const oldTop = promptYRef.current;

    // Collapse the textarea to its final (short) height first so we measure the
    // true docked target position (submit() deferred this exact reset for us).
    if (shouldCollapse && ta) ta.style.height = '';
    const shortTop = el.getBoundingClientRect().top;

    const oldHeight = promptTaHeightRef.current;
    const newHeight = ta ? ta.offsetHeight : 0;
    const animateHeight =
      shouldCollapse && !!ta && oldHeight !== null && oldHeight - newHeight >= 5;

    const shortDelta = oldTop - shortTop;
    if (Math.abs(shortDelta) < 5 && !animateHeight) return;

    // Gate content while prompt animates — compose→thread gates ThreadView
    promptAnimating.value = true;

    // Invert. When also animating height, restore the tall height first and
    // recompute the offset against that taller docked layout — with matched
    // duration+easing the transform and height then interpolate old→new in
    // lockstep, so the visual top slides smoothly as the box shrinks.
    let delta = shortDelta;
    if (animateHeight && ta && oldHeight !== null) {
      ta.style.transition = 'none';
      ta.style.height = `${oldHeight}px`;
      delta = oldTop - el.getBoundingClientRect().top;
    }
    el.style.transition = 'none';
    el.style.transform = `translateY(${delta}px)`;

    const clearAnimation = () => {
      clearTimeout(safetyTimer);
      el.removeEventListener('transitionend', onTransitionEnd);
      el.style.transition = '';
      el.style.transform = '';
      // Only undo the inline height the height-collapse animation set. On a
      // position-only FLIP (e.g. navigating INTO a draft) the textarea height is
      // owned by autoResize — which sizes it to the draft's text — so clearing it
      // here would snap a multi-line draft back to min-height after it settled.
      if (animateHeight) resetTaHeight();
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
        if (animateHeight && ta) {
          ta.style.transition = 'height 0.3s ease';
          ta.style.height = `${newHeight}px`;
        }
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
      <div class="thread-pane-body">
        {tid && !isComposingDraft ? <div class="thread-view-clip"><ThreadView /></div> : <CreateThreadView />}
        <div class="prompt-area" ref={promptRef}>
          <PromptInput />
        </div>
      </div>
    </div>
  );
}
