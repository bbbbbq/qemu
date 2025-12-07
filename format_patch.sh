git format-patch HEAD~1 \
    --subject-prefix="PATCH" \
    --thread \
    --cover-letter \
    -s \
    -o ./patches