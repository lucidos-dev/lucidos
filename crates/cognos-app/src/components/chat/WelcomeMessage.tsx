export function WelcomeMessage() {
  return (
    <div class="response-content markdown-content welcome-message">
      <h2>Welcome to Lucidos</h2>
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
        <p class="drop-hint">Drop files anywhere to import them</p>
      </div>
    </div>
  );
}
