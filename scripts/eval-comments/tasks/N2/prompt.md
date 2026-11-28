A Codex coding-agent thread asked me a question while a command it had
started earlier was still running. When that command finished, the thread
stopped showing as waiting for my answer and showed as running again. The
question card was still there, so the UI said "Needs your answer" right
under a live Stop button.

It gets worse if I take a while to answer. The hang watchdog killed the
session, which it should not do while the agent is waiting on me. Then the
session resumed on its own and the agent asked me the same question again.
I saw the same thing with a permission card in place of the question.

A thread that is waiting for my answer should keep waiting until I answer,
whatever earlier tool calls finish in the meantime. The question's own
result should still end the wait, for example when the question is
dismissed. After I answer, new tool results should show the thread as
running, as they do now.
