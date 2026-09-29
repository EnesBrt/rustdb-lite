#!/usr/bin/env python3
"""Check encoding-sensitive SQL and preservation of native indexed UTF-16 files."""
import json
from pathlib import Path
import subprocess
import tempfile
from sql_differential import ROOT,ORACLE,run_engine
from index_differential import native


def expected(path,query,count,encoding):
    fields=[]
    for i in range(count):
        c=f'c{i}'
        fields.extend([f'typeof({c}) AS t{i}',f"CASE typeof({c}) WHEN 'text' THEN hex({c}) WHEN 'blob' THEN hex({c}) WHEN 'real' THEN hex(ieee754_to_blob({c})) ELSE {c} END AS v{i}"])
    raw=native(path,'SELECT '+','.join(fields)+' FROM ('+query+');')
    rows=[]
    for row in raw:
        values=[]
        for i in range(count):
            kind=row[f't{i}'];value=row[f'v{i}'];item={'type':kind}
            if kind=='integer':item['value']=value
            elif kind=='real':item['bits']=value
            elif kind in ('text','blob'):
                item['hex']=bytes.fromhex(value).decode(encoding).encode('utf-8').hex().upper() if kind=='text' else value
            values.append(item)
        rows.append(values)
    return rows


def main():
    subprocess.run(['cargo','build','--bin','sqlite-safe-sql'],cwd=ROOT,check=True)
    cases=0
    with tempfile.TemporaryDirectory(prefix='safe-encoding-') as directory:
        for encoding in ['UTF-8','UTF-16le','UTF-16be']:
            path=Path(directory)/f'{encoding}.db'
            native(path,f"PRAGMA encoding='{encoding}';PRAGMA application_id=1196444487;PRAGMA user_version=123;CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT,n,b BLOB);CREATE INDEX idx ON t(v DESC);INSERT INTO t VALUES(1,'a',12,x'31003200'),(2,'Ā',2.5,x'41004200'),(3,'🦀',NULL,x'0001'),(4,'\ue000',-1,x'00'),(5,'A',0,NULL),(6,'a ',9,x'3132');")
            queries=[
                ('SELECT v c0 FROM t ORDER BY v',1),
                ('SELECT v c0 FROM t ORDER BY v DESC',1),
                ('SELECT v c0 FROM t ORDER BY v COLLATE NOCASE',1),
                ('SELECT v c0,count(*) c1 FROM t GROUP BY v ORDER BY v',2),
                ('SELECT min(v) c0,max(v) c1 FROM t',2),
                ("SELECT min('a','Ā','🦀') c0,max('a','Ā','🦀') c1",2),
                ("SELECT v c0 FROM t WHERE v<'a' ORDER BY id",1),
                ("SELECT 'Ā'<'a' c0,'🦀'<'a' c1",2),
                ("SELECT v c0,CASE v WHEN 'Ā' THEN 1 ELSE 0 END c1 FROM t ORDER BY id",2),
                ('SELECT hex(v) c0,hex(CAST(v AS BLOB)) c1,octet_length(v) c2,length(v) c3 FROM t ORDER BY id',4),
                ('SELECT hex(substr(v,1)) c0,hex(upper(v)) c1 FROM t ORDER BY id',2),
                ('SELECT hex(n) c0,hex(CAST(n AS BLOB)) c1,octet_length(n) c2 FROM t ORDER BY id',3),
                ('SELECT hex(char(256)) c0,hex(lower(char(256))) c1',2),
                ('SELECT hex(CAST(CAST(v AS BLOB) AS TEXT)) c0 FROM t ORDER BY id',1),
                ('SELECT octet_length(b) c0,hex(b) c1 FROM t ORDER BY id',2),
                ("SELECT hex(CAST(12 AS BLOB)) c0,CAST(CAST(12 AS BLOB) AS INTEGER) c1",2),
                ("SELECT hex(CAST(12.5 AS BLOB)) c0,CAST(CAST(12.5 AS BLOB) AS NUMERIC) c1",2),
            ]
            for query,count in queries:
                actual=run_engine(query+';','--load',path)[-1]['rows']
                want=expected(path,query,count,encoding)
                assert actual==want,(encoding,query,actual,want)
                cases+=1
            reexport=Path(directory)/f'export-{encoding}.db'
            run_engine("UPDATE t SET v=v||'🦀';",'--load',path,'--save',reexport)
            assert native(reexport,'PRAGMA integrity_check;')==[{'integrity_check':'ok'}]
            assert native(reexport,'PRAGMA encoding;')==[{'encoding':encoding}]
            assert native(reexport,'PRAGMA application_id;')==[{'application_id':1196444487}]
            assert native(reexport,'PRAGMA user_version;')==[{'user_version':123}]
            native(path,"UPDATE t SET v=v||'🦀';")
            assert native(path,'SELECT * FROM t ORDER BY v;')==native(reexport,'SELECT * FROM t ORDER BY v;')
            cases+=1
    print(f'PASS: {cases} encoding-sensitive SQL/metadata/index scenarios against SQLite 3.53.4')

if __name__=='__main__':main()
