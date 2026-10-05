// An agent's tools by the tool set they come from (issue 194): each set
// with its name, what it is for and, if it is unavailable, why; each tool
// with its description, marked where the runtime treats its calls
// specially, and, unfolded, its parameters. Shown for the tools a message
// was offered (ContextPanel) and for those an agent has now (ToolsPanel).

import type {Tool, ToolSet} from '../types';
import {classificationLabel} from '../confidentiality';

/** What the runtime acts on of `tool`, as marks after its name. */
function marksOf(tool: Tool): string[] {
  const marks: string[] = [];
  if (tool.requires_approval) marks.push('needs approval');
  if (tool.visibility?.call === 'owner') marks.push('call owner only');
  else if (tool.visibility?.content === 'owner') marks.push('content owner only');
  if (tool.classification) marks.push(classificationLabel(tool.classification));
  return marks;
}

export function ToolSetList({sets}: {sets: ToolSet[]}) {
  return (
    <div className="space-y-3">
      {sets.map((set, index) => (
        <div key={set.name || index} className="space-y-1">
          {set.name && (
            <div className="text-xs">
              <span className="font-semibold text-gray-700 dark:text-gray-200">{set.name}</span>
              {set.description && <span className="text-gray-500 dark:text-gray-400"> - {set.description}</span>}
            </div>
          )}
          {set.unavailable && (
            <div className="text-xs text-amber-600 dark:text-amber-400">Unavailable: {set.unavailable}</div>
          )}
          <div className={set.name ? 'ml-3 space-y-2' : 'space-y-2'}>
            {set.tools.map(tool => (
              <details key={tool.name}>
                <summary className="cursor-pointer text-xs">
                  <code className="font-mono font-semibold text-gray-800 dark:text-gray-100">{tool.name}</code>
                  {marksOf(tool).map(mark => (
                    <span key={mark} className="ml-1 px-1 rounded bg-gray-100 dark:bg-gray-800 text-[10px] text-gray-600 dark:text-gray-300">{mark}</span>
                  ))}
                  <span className="text-gray-500 dark:text-gray-400"> - {tool.description}</span>
                </summary>
                <pre className="mt-1 whitespace-pre-wrap text-[11px] text-gray-600 dark:text-gray-300">
                  {JSON.stringify(tool.parameters, null, 2)}
                </pre>
              </details>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
