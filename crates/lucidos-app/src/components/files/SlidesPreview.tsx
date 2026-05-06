import { stripHtml } from '../../utils/escapeHtml';

interface SlidesDeck {
  id?: string;
  title: string;
  subtitle?: string;
  sections?: Section[];
  slides?: Slide[];
}

interface Section {
  title: string;
  color?: string;
  slides: Slide[];
}

interface Slide {
  title: string;
  hero?: boolean;
  content?: ContentNode[];
}

interface ContentNode {
  type: string;
  tag?: { text: string; color?: string };
  title?: string;
  subtitle?: string;
  text?: string;
  content?: string;
  emoji?: string;
  label?: string;
  num?: number;
  body?: string;
  highlight?: string;
  items?: string[];
  events?: string[];
  chips?: string[];
  icon?: string;
  color?: string;
  children?: ContentNode[];
}

interface FlatSlide {
  slide: Slide;
  sectionTitle?: string;
  sectionColor?: string;
  globalIndex: number;
}

function flattenSlides(deck: SlidesDeck): FlatSlide[] {
  const result: FlatSlide[] = [];
  let idx = 1;
  if (deck.sections) {
    for (const section of deck.sections) {
      for (const slide of section.slides) {
        result.push({ slide, sectionTitle: section.title, sectionColor: section.color, globalIndex: idx++ });
      }
    }
  }
  if (deck.slides) {
    for (const slide of deck.slides) {
      result.push({ slide, globalIndex: idx++ });
    }
  }
  return result;
}

function renderNode(node: ContentNode, key: number): preact.JSX.Element | null {
  switch (node.type) {
    case 'slideHeader':
      return (
        <div class="slides-pv-header" key={key}>
          {node.tag && <span class="slides-pv-tag" data-color={node.tag.color}>{node.tag.text}</span>}
          <div class="slides-pv-header-title">{node.title}</div>
          {node.subtitle && <div class="slides-pv-header-subtitle">{stripHtml(node.subtitle)}</div>}
        </div>
      );

    case 'html':
      return <div class="slides-pv-html" key={key} dangerouslySetInnerHTML={{ __html: node.content || '' }} />;

    case 'list':
      return (
        <ul class="slides-pv-list" key={key}>
          {node.items?.map((item, i) => <li key={i} dangerouslySetInnerHTML={{ __html: item }} />)}
        </ul>
      );

    case 'icon':
      return <div class="slides-pv-icon" key={key}>{node.emoji}</div>;

    case 'insight':
      return (
        <div class="slides-pv-insight" data-color={node.color} key={key}>
          <span dangerouslySetInnerHTML={{ __html: node.text || '' }} />
        </div>
      );

    case 'card':
      return (
        <div class="slides-pv-card" data-highlight={node.highlight} key={key}>
          {node.children?.map((child, i) => renderNode(child, i))}
        </div>
      );

    case 'columns':
    case 'threeCol':
    case 'fourCol':
      return (
        <div class="slides-pv-columns" data-layout={node.type} key={key}>
          {node.children?.map((child, i) => renderNode(child, i))}
        </div>
      );

    case 'group':
      return (
        <div class="slides-pv-group" key={key}>
          {node.children?.map((child, i) => renderNode(child, i))}
        </div>
      );

    case 'pipeline':
      return (
        <div class="slides-pv-pipeline" key={key}>
          {node.children?.map((child, i) => renderNode(child, i))}
        </div>
      );

    case 'pipelineStep':
      return (
        <div class="slides-pv-pipeline-step" key={key}>
          <span class="slides-pv-pipeline-icon">{node.icon}</span>
          <span class="slides-pv-pipeline-label">{node.label}</span>
        </div>
      );

    case 'tree':
      return <pre class="slides-pv-tree" key={key} dangerouslySetInnerHTML={{ __html: node.content || '' }} />;

    case 'skillChips':
      return (
        <div class="slides-pv-chips" key={key}>
          {node.chips?.map((chip, i) => <span class="slides-pv-chip" key={i}>{chip}</span>)}
        </div>
      );

    case 'esFlow':
      return (
        <div class="slides-pv-es-flow" key={key}>
          {node.events?.map((ev, i) => (
            <span key={i}>
              <span class="slides-pv-es-event">{ev}</span>
              {i < (node.events?.length || 0) - 1 && <span class="slides-pv-es-arrow">{'\u2192'}</span>}
            </span>
          ))}
        </div>
      );

    case 'takeawayList':
      return (
        <div class="slides-pv-takeaways" key={key}>
          {node.children?.map((child, i) => renderNode(child, i))}
        </div>
      );

    case 'takeawayItem':
      return (
        <div class="slides-pv-takeaway" key={key}>
          <span class="slides-pv-takeaway-num">{node.num}</span>
          <div>
            <div class="slides-pv-takeaway-title">{node.title}</div>
            {node.body && <div class="slides-pv-takeaway-body" dangerouslySetInnerHTML={{ __html: node.body }} />}
          </div>
        </div>
      );

    case 'teamBadge':
      return <div class="slides-pv-badge" key={key}>{node.text}</div>;

    case 'vsLabel':
      return <div class="slides-pv-vs-label" key={key}>{node.text}</div>;

    case 'archDiagram':
      return <div class="slides-pv-diagram" key={key} dangerouslySetInnerHTML={{ __html: node.content || '' }} />;

    default:
      return null;
  }
}

interface Props {
  content: string;
}

export function SlidesPreview({ content }: Props) {
  let deck: SlidesDeck;
  try {
    deck = JSON.parse(content);
  } catch {
    return <div class="empty-state error-text">Invalid .slides JSON</div>;
  }

  const slides = flattenSlides(deck);

  return (
    <div class="slides-pv">
      <div class="slides-pv-deck-header">
        <div class="slides-pv-deck-title">{deck.title}</div>
        {deck.subtitle && <div class="slides-pv-deck-subtitle">{deck.subtitle}</div>}
        <div class="slides-pv-deck-count">{slides.length} slides</div>
      </div>
      {slides.map(({ slide, sectionTitle, sectionColor, globalIndex }) => (
        <div class="slides-pv-slide" key={globalIndex}>
          <div class="slides-pv-slide-header">
            <span class="slides-pv-slide-num">{globalIndex}</span>
            <span class="slides-pv-slide-title">{slide.title}</span>
            {slide.hero && <span class="slides-pv-hero-badge">Hero</span>}
            {sectionTitle && <span class="slides-pv-section-badge" data-color={sectionColor}>{sectionTitle}</span>}
          </div>
          {slide.content && (
            <div class="slides-pv-slide-body">
              {slide.content.map((node, i) => renderNode(node, i))}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
