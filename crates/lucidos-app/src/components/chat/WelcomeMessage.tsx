export function WelcomeMessage() {
  return (
    <div class="response-content markdown-content welcome-message">
      <h2>Welcome to Lucidos</h2>
      <p class="tagline">If You Can Describe It, It Exists.</p>
      <p>
        I'm your personal cognitive assistant. I remember our conversations,
        track your projects, and help you stay organized.
      </p>
      <div class="getting-started">
        <p>
          <strong>Start by telling me:</strong>
        </p>
        <ul>
          <li>"I'm starting a new project called..."</li>
          <li>"Help me plan my week"</li>
          <li>"Remind me every morning at 8am to..."</li>
        </ul>
      </div>
      <p class="disclaimer">
        Provided as is under the{' '}
        <a
          href="https://github.com/lucidos-dev/lucidos/blob/main/LICENSE"
          target="_blank"
          rel="noopener noreferrer"
        >
          MIT license
        </a>
        . No warranty, and no liability for actions performed or their
        consequences.
      </p>
    </div>
  );
}
