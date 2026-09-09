/**
 * The verbs Home offers on a project, in one place: the card's context menu,
 * the chapter view's header menu and the keyboard shortcut all raise the same
 * dialogs and call the same functions.
 *
 * Dialogs go on the global modal stack rather than being mounted inside the
 * screen, because `N` (new project) is a global shortcut that pushes
 * `{kind: 'newProject'}` from `src/lib/shortcuts.js` - the stack is already the
 * way in, so Home's own buttons use it too and there is exactly one path.
 */

import { pushModal, openProject } from '../state/app.svelte.js'
import { t } from '../i18n/index.js'
import { copySourcePath, removeChapter, removeProject } from './library.svelte.js'

/** Raise the New project dialog. */
export function openNewProject() {
  pushModal({ kind: 'newProject' })
}

/**
 * Raise the New chapter dialog, optionally with a project preselected.
 * @param {string} [projectId]
 */
export function openNewChapter(projectId) {
  pushModal({ kind: 'newChapter', props: { projectId: projectId ?? null } })
}

/** @param {import('../api/backend.js').ApiProject} project */
export function openRenameProject(project) {
  pushModal({ kind: 'renameProject', props: { projectId: project.id } })
}

/**
 * Removing a project drops it from the library; the pages on disk are not
 * touched, and the copy has to say so or the user cannot tell what they are
 * agreeing to.
 *
 * @param {import('../api/backend.js').ApiProject} project
 */
export function confirmRemoveProject(project) {
  pushModal({
    kind: 'removeProject',
    props: {
      bodyKey: 'home.remove.body',
      bodyParams: { name: project.name, path: project.sourcePath },
    },
    actions: [
      { id: 'cancel', labelKey: 'shell.action.cancel' },
      { id: 'remove', labelKey: 'home.action.remove', variant: 'primary' },
    ],
    onresolve: (result) => {
      if (result === 'remove') removeProject(project.id)
    },
  })
}

/**
 * Delete a chapter, after asking what "delete" is to mean.
 *
 * Two destructive answers rather than one, because the two are not the same
 * act: dropping the chapter from the library leaves the scans where they are
 * and can be undone by adding the chapter again, while taking the scans as well
 * destroys files this app did not make and cannot put back. The dialog names
 * the folder, so the second button is never pressed without its path in view.
 *
 * The safe answer is the primary one. The backend refuses to remove the
 * project's own folder whichever button is pressed - see the seam's
 * `deleteChapter` - so this cannot empty the project even by mistake.
 *
 * @param {import('../api/backend.js').ApiProject} project
 * @param {import('../api/backend.js').ApiChapter} chapter
 */
export function confirmDeleteChapter(project, chapter) {
  const ownFolder = !chapter.sourcePath || chapter.sourcePath === project.sourcePath
  pushModal({
    kind: 'deleteChapter',
    props: {
      bodyKey: ownFolder ? 'home.deleteChapter.bodyOwnFolder' : 'home.deleteChapter.body',
      bodyParams: { number: chapter.number, name: chapter.name, path: chapter.sourcePath },
    },
    actions: [
      { id: 'cancel', labelKey: 'shell.action.cancel' },
      ...(ownFolder ? [] : [{ id: 'scans', labelKey: 'home.deleteChapter.withScans' }]),
      { id: 'chapter', labelKey: 'home.deleteChapter.keepScans', variant: 'primary' },
    ],
    onresolve: (result) => {
      if (result !== 'chapter' && result !== 'scans') return
      removeChapter({
        projectId: project.id,
        chapterId: chapter.id,
        sourceFiles: result === 'scans',
      })
    },
  })
}

/**
 * The context menu for one chapter. One verb today, and it is the destructive
 * one - the row itself is how a chapter is opened.
 *
 * @returns {Array<Object>}
 */
export function chapterMenuItems() {
  return [{ id: 'delete', label: t('home.action.deleteChapter'), icon: 'trash' }]
}

/**
 * @param {string} id - the item selected from `chapterMenuItems`
 * @param {import('../api/backend.js').ApiProject} project
 * @param {import('../api/backend.js').ApiChapter} chapter
 */
export function runChapterAction(id, project, chapter) {
  if (id === 'delete') confirmDeleteChapter(project, chapter)
}

/**
 * The context menu for one project. Ordered by how often it is reached, with
 * the destructive entry alone at the bottom behind a separator.
 *
 * @param {import('../api/backend.js').ApiProject} project
 * @returns {Array<Object>}
 */
export function projectMenuItems(project) {
  return [
    { id: 'open', label: t('home.action.open'), icon: 'folder' },
    { id: 'newChapter', label: t('home.action.newChapter'), icon: 'plus' },
    { id: 'sep-1', separator: true },
    { id: 'rename', label: t('home.action.rename') },
    { id: 'copyPath', label: t('home.action.copySourcePath') },
    { id: 'sep-2', separator: true },
    { id: 'remove', label: t('home.action.remove'), icon: 'trash' },
  ]
}

/**
 * @param {string} id - the item selected from `projectMenuItems`
 * @param {import('../api/backend.js').ApiProject} project
 */
export function runProjectAction(id, project) {
  switch (id) {
    case 'open':
      openProject(project.id)
      break
    case 'newChapter':
      openNewChapter(project.id)
      break
    case 'rename':
      openRenameProject(project)
      break
    case 'copyPath':
      copySourcePath(project)
      break
    case 'remove':
      confirmRemoveProject(project)
      break
    default:
  }
}
