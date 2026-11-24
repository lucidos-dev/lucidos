import { useRef, useLayoutEffect } from 'preact/hooks';
import { focusedThreadId, promptAnimating, promptSendCollapsing, activeThreadIsComposing, composeViewActive, homeWelcomeActive, scaledDurationMs } from '../../store/store';
import { PromptInput } from '../chat/PromptInput';
import { FollowLiveEdgeToggle } from '../chat/FollowToggle';
import { CreateThreadView } from '../chat/CreateThreadView';
import { ThreadView } from '../chat/ThreadView';
import { HomeWelcomeTitle } from '../chat/HomeWelcomeTitle';
import { isReducedMotion } from '../../utils/motion';
import { isMobile } from '../../utils/viewport';

const PROMPT_FLIP_MS = 300;

export function ThreadPane() {
  const tid = focusedThreadId.value;
  const isComposingDraft = activeThreadIsComposing.value;
  const isHomeWelcome = homeWelcomeActive.value;
  const isComposeEmpty = composeViewActive.value;

  const promptRef = useRef<HTMLDivElement>(null);
  const promptYRef = useRef<number | null>(null);
  const promptTaHeightRef = useRef<number | null>(null);
  const prevComposeEmpty = useRef(isComposeEmpty);

  if (promptRef.current && prevComposeEmpty.current !== isComposeEmpty) {
    promptYRef.current = promptRef.current.getBoundingClientRect().top;
    const ta = promptRef.current.querySelector<HTMLElement>('.prompt-textarea');
    promptTaHeightRef.current = ta ? ta.offsetHeight : null;
  }

  useLayoutEffect(() => {
    if (prevComposeEmpty.current === isComposeEmpty) return;
    const leavingCompose = !isComposeEmpty;
    prevComposeEmpty.current = isComposeEmpty;

    const collapsing = promptSendCollapsing.peek();
    if (collapsing) promptSendCollapsing.value = false;
    const shouldCollapse = collapsing && leavingCompose;

    const el = promptRef.current;
    const ta = el?.querySelector<HTMLElement>('.prompt-textarea') ?? null;
    const resetTaHeight = () => { if (ta) { ta.style.transition = ''; ta.style.height = ''; } };

    if (!el || promptYRef.current === null) {
      if (shouldCollapse) resetTaHeight();
      return;
    }

    if (isReducedMotion() || isMobile()) {
      if (shouldCollapse) resetTaHeight();
      return;
    }

    const oldTop = promptYRef.current;

    if (shouldCollapse && ta) ta.style.height = '';
    const shortTop = el.getBoundingClientRect().top;

    const oldHeight = promptTaHeightRef.current;
    const newHeight = ta ? ta.offsetHeight : 0;
    const animateHeight =
      shouldCollapse && !!ta && oldHeight !== null && oldHeight - newHeight >= 5;

    const shortDelta = oldTop - shortTop;
    if (Math.abs(shortDelta) < 5 && !animateHeight) return;

    promptAnimating.value = true;

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
      if (animateHeight) resetTaHeight();
      promptAnimating.value = false;
    };

    const onTransitionEnd = (e: TransitionEvent) => {
      if (e.target !== el || e.propertyName !== 'transform') return;
      clearAnimation();
    };

    let safetyTimer = setTimeout(clearAnimation, scaledDurationMs(PROMPT_FLIP_MS) + 100);

    let raf1: number | undefined;
    let raf2: number | undefined;
    raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => {
        el.addEventListener('transitionend', onTransitionEnd);
        const flipMs = scaledDurationMs(PROMPT_FLIP_MS);
        el.style.transition = `transform ${flipMs}ms ease`;
        el.style.transform = '';
        if (animateHeight && ta) {
          ta.style.transition = `height ${flipMs}ms ease`;
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
      {tid && isHomeWelcome && <HomeWelcomeTitle key="home-title" threadId={tid} />}
      <div key="body" class="thread-pane-body">
        {tid && !isComposingDraft && !isHomeWelcome
          ? <div key="thread" class="thread-view-clip"><ThreadView /></div>
          : <CreateThreadView key="compose" />}
        <div key="prompt" class="prompt-area" ref={promptRef}>
          <FollowLiveEdgeToggle />
          <PromptInput />
        </div>
      </div>
    </div>
  );
}
