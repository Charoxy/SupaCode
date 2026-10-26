# ide/ : l'éditeur SupaCode

L'IDE SupaCode est construit sur [VSCodium](https://github.com/VSCodium/vscodium), la distribution libre de VS Code.

## Le principe : des patchs, pas une copie

On **ne copie pas** les sources de VS Code dans ce dépôt. On garde uniquement :

- `scripts/fetch-vscodium.sh` : clone VSCodium (`--depth 1`) dans `ide/vscodium/`, ou le met à jour ;
- `patches/*.patch` : nos modifications, appliquées dans l'ordre alphabétique après la récupération.

Ainsi, suivre l'amont revient à relancer le script : on récupère la dernière version de VSCodium et on réapplique nos patchs. Le dossier `ide/vscodium/` est ignoré par git.

```bash
./ide/scripts/fetch-vscodium.sh                 # branche master
VSCODIUM_REF=1.99.0 ./ide/scripts/fetch-vscodium.sh   # une version précise
```

Pour créer un patch, modifie les fichiers dans `ide/vscodium/`, puis :

```bash
git -C ide/vscodium diff > ide/patches/0001-mon-changement.patch
```

## Prochaines étapes

1. **Branding** : nom, icônes, écran d'accueil et identifiants SupaCode, sous forme de patchs.
2. **Extension intégrée** : une extension livrée avec l'IDE qui embarque `@supacode/core` (orchestrateur, panneau de suivi des agents, choix manuel de l'agent et du modèle).
