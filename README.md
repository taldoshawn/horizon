# Horizon Android — experimento Minecraft Java 1.8.9

Fork de desenvolvimento do [Horizon Android](https://github.com/Vasysik/horizon-android), com interface Rust inspirada na organização do PojavLauncher e pesquisa de compatibilidade Minecraft Java 1.8.9.

## Estado

- O motor atual do Horizon **continua baseado na versão 26.3**.
- A conexão com servidores 1.8.9 está sendo estudada com o protocolo 47; os codecs ainda não estão integrados ao motor completo de jogo.
- A opção existente de conexão 1.8.9 usa **ViaProxy externo** (por exemplo, Termux no Android).
- **Não é ainda um Minecraft 1.8.9 nativo jogável**. O APK é experimental.

## Preparação automática do repositório

Este repositório começa com um patch testado contra o commit `6eca278fd92e99ade83ae5823d090cb4bdc6d046` da branch `android` do projeto original.

O workflow [Importar código-fonte](.github/workflows/bootstrap.yml) faz clone da base original, aplica `patches/horizon-android-189-pojav.patch` e envia o código completo para a branch `main`.

Depois, o workflow [Android dev APKs](.github/workflows/android-dev.yml) tenta gerar um APK arm64-v8a na aba **Actions**. Para baixar, abra a execução e procure o arquivo em **Artifacts**. É necessário que o Actions esteja habilitado e tenha permissão de escrita para a importação.

Uma compilação concluída significa somente que o **APK experimental** foi gerado. Não prova compatibilidade 1.8.9.

## Licença e créditos

Respeitar as licenças, créditos e avisos do projeto original. Os assets proprietários do Minecraft não são distribuídos neste repositório; a preparação deve usar fontes autorizadas.
